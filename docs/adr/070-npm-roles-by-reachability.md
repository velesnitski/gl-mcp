# ADR 070: npm runtime, build and dev roles by reachability

## Status

Accepted (2026-10-26)

## Context

The weekly dependency email (ADR 068) ranks repositories by runtime exposure.
Frontend repositories came out with dozens of "runtime" critical findings in
packages such as `loader-utils`, `shell-quote`, `tar` and `websocket-driver`. These
are webpack and dev-server internals that never reach a user. A frontend lead
correctly answered that most of them only run in the Node.js build environment.

The cause was the lockfile's own `dev` flag, which answers the wrong question twice:

- yarn v1 and older pnpm lockfiles do not record it, so everything read as runtime;
- frontend projects often list webpack, babel or vite under `dependencies`, so even
  npm's flag calls their whole toolchain production code.

At the same time, some findings in those repositories *do* ship to the browser
(UI libraries, HTTP clients, utility libraries). "It all runs in the browser, so
it's fine" is not right either. The report needs the actual split.

## Decision

1. **Roles come from reachability on the lockfile's package graph.**
   - The edges are parsed from package-lock v1–v3, yarn v1 and berry, and pnpm
     v5–v9 (`packages`/`snapshots`).
   - The roots are the declared dependencies. They come from the lockfile root or
     its importers. Where the lockfile doesn't record them (yarn, package-lock v1),
     they come from the `package.json` beside it, which costs one extra request.
2. **Declared build tooling becomes a build root, wherever it was declared.** The
   tooling is a fixed list: bundlers, compilers, linters, CSS tooling, test
   runners, and `@types`.
   - **runtime**: reachable from the remaining runtime roots;
   - **build**: reachable only through build roots;
   - **dev**: reachable only from `devDependencies`.

   Runtime wins over build, and build wins over dev.
3. **The graph is by package name.** A name reached at any version from a runtime
   root counts as runtime. This over-counts runtime slightly and never under-counts
   it, which is the safe direction for a security report.
4. **No roots known** (no root entry and no `package.json`) means the lockfile's own
   flags stand, as before.
5. **Reporting:**
   - Build-tooling findings are labelled "· build", like "· dev".
   - They weigh a quarter in the exposure ranking and do not count as urgent.
   - The weekly snapshot gains a `build` field. Older snapshots load with it false.

## Consequences

- Frontend findings now separate what can ship to users from what runs at build
  time. Build-time vulnerabilities still show up, ranked lower: CI runners and
  developer machines are attack surface too.
- "Runtime" for a browser app still means "can be bundled", not "is bundled". Only
  the production bundle settles that, and the report does not inspect bundles.
- The build-tool list is a judgement. A package missing from it is treated as
  runtime, which errs towards over-reporting.
- **Known blind spot: bundler-injected polyfills.** Webpack 4 (`node-libs-browser`)
  bundles `crypto-browserify` and its chain (`elliptic`, `pbkdf2`, `sha.js`,
  `cipher-base`, `browserify-sign`) into the browser build when app code imports a
  Node core module such as `crypto`. In the lockfile these are reachable only
  through webpack, so they are labelled dev or build. Whether they ship depends on
  the app's imports, which a lockfile cannot show. Checked on a real webpack 4
  landing project, the runtime set otherwise matched a manual review (UI library,
  HTTP client and its redirect handler). For webpack 4 projects, check the bundle or
  the imports before discounting those packages.
