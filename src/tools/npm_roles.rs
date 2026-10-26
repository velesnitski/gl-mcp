//! Which npm packages actually run, and which only build or test the app.
//!
//! A lockfile's own `dev` flag answers "was it declared under devDependencies", which
//! is the wrong question twice over: yarn v1 and older pnpm locks don't record it at
//! all, and frontend projects routinely list webpack, babel or vite under
//! `dependencies`. Everything those tools pull in then looks like production code,
//! and a dev-server CVE reads as a runtime one.
//!
//! So roles come from reachability on the lockfile's package graph:
//! - roots are the project's declared dependencies (lockfile root/importers, or the
//!   package.json next to it);
//! - declared dependencies that are build tooling become *build* roots, wherever they
//!   were declared;
//! - runtime = reachable from the remaining runtime roots; build = reachable only
//!   from build roots; dev = reachable only from devDependencies.
//!
//! The graph is by package name, not name@version: a name reached at any version from
//! a runtime root counts as runtime. That over-counts runtime slightly and never
//! under-counts it — the safe direction for a security report.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Role {
    Runtime,
    Build,
    Dev,
}

/// Build and test tooling: what turns source into a bundle, checks it or tests it.
/// Exact names, or prefixes ending in '/' or '-'.
const BUILD_TOOLS: &[&str] = &[
    "webpack", "webpack-", "vite", "vite-plugin-", "@vitejs/", "unplugin-", "rollup", "rollup-plugin-", "@rollup/", "esbuild", "parcel", "@parcel/", "turbo", "@swc/", "swc-loader",
    "@babel/", "babel-", "typescript", "ts-loader", "ts-node", "tsx", "tsup",
    "eslint", "eslint-", "@eslint/", "@typescript-eslint/", "prettier", "stylelint", "stylelint-",
    "postcss", "postcss-", "autoprefixer", "tailwindcss", "sass", "node-sass", "less", "less-loader",
    "sass-loader", "css-loader", "style-loader", "file-loader", "url-loader", "raw-loader", "vue-loader",
    "html-webpack-plugin", "mini-css-extract-plugin", "copy-webpack-plugin", "terser", "terser-webpack-plugin",
    "css-minimizer-webpack-plugin", "@vue/cli", "@vue/cli-", "vue-template-compiler", "@angular-devkit/",
    "@angular/cli", "react-scripts", "gulp", "gulp-", "grunt", "grunt-", "browserify",
    "jest", "jest-", "@jest/", "vitest", "@vitest/", "karma", "karma-", "mocha", "chai", "cypress",
    "@playwright/", "playwright", "@storybook/", "storybook", "@testing-library/",
    "nodemon", "concurrently", "husky", "lint-staged", "rimraf", "cross-env", "@types/",
];

pub(crate) fn is_build_tool(name: &str) -> bool {
    BUILD_TOOLS.iter().any(|t| {
        if t.ends_with('/') || t.ends_with('-') {
            name.starts_with(t)
        } else {
            name == *t
        }
    })
}

/// Declared roots of a project, by kind.
#[derive(Debug, Default, Clone)]
pub(crate) struct Roots {
    pub runtime: BTreeSet<String>,
    pub dev: BTreeSet<String>,
}

/// Name-level dependency graph plus the roots the lockfile itself declares.
#[derive(Debug, Default)]
pub(crate) struct Graph {
    pub edges: BTreeMap<String, BTreeSet<String>>,
    pub roots: Roots,
}

fn object_keys(v: &Value) -> impl Iterator<Item = String> + '_ {
    v.as_object().into_iter().flat_map(|o| o.keys().cloned())
}

/// Roots from a package.json (`dependencies`, `optionalDependencies`, `peerDependencies`
/// run; `devDependencies` don't).
pub(crate) fn roots_from_package_json(text: &str) -> Option<Roots> {
    let doc: Value = serde_json::from_str(text).ok()?;
    let mut r = Roots::default();
    for k in ["dependencies", "optionalDependencies", "peerDependencies"] {
        r.runtime.extend(object_keys(&doc[k]));
    }
    r.dev.extend(object_keys(&doc["devDependencies"]));
    Some(r)
}

/// Name from a node_modules path ("node_modules/a/node_modules/@s/b" → "@s/b").
fn name_from_path(path: &str) -> Option<&str> {
    path.rfind("node_modules/").map(|i| &path[i + "node_modules/".len()..])
}

pub(crate) fn graph_package_lock(text: &str) -> Option<Graph> {
    let doc: Value = serde_json::from_str(text).ok()?;
    let mut g = Graph::default();
    if let Some(packages) = doc["packages"].as_object() {
        for (path, info) in packages {
            let deps = ["dependencies", "optionalDependencies", "peerDependencies"]
                .iter()
                .flat_map(|k| object_keys(&info[*k]).collect::<Vec<_>>());
            match name_from_path(path) {
                Some(name) => {
                    let name = info["name"].as_str().unwrap_or(name).to_string();
                    g.edges.entry(name).or_default().extend(deps);
                }
                // "" is the root project; other paths without node_modules are workspaces.
                None => {
                    g.roots.runtime.extend(deps);
                    g.roots.dev.extend(object_keys(&info["devDependencies"]));
                }
            }
        }
    } else {
        // v1: nested "dependencies" with "requires"; roots come from package.json.
        fn walk(deps: &Value, g: &mut Graph) {
            for (name, info) in deps.as_object().into_iter().flatten() {
                g.edges.entry(name.clone()).or_default().extend(object_keys(&info["requires"]));
                walk(&info["dependencies"], g);
            }
        }
        walk(&doc["dependencies"], &mut g);
    }
    Some(g)
}

/// yarn v1 and berry: entry headers at column 0, `dependencies:` blocks inside.
pub(crate) fn graph_yarn_lock(text: &str) -> Graph {
    let mut g = Graph::default();
    let mut current: Option<String> = None;
    let mut in_deps = false;
    for line in text.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') {
            let first = line.trim_end_matches(':').split(',').next().unwrap_or("").trim().trim_matches('"');
            let at = first.get(1..).and_then(|rest| rest.find('@')).map(|i| i + 1);
            current = at.map(|i| first[..i].to_string());
            in_deps = false;
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if indent <= 2 {
            in_deps = matches!(t, "dependencies:" | "optionalDependencies:" | "peerDependencies:");
            continue;
        }
        if let (true, Some(name)) = (in_deps, &current) {
            // v1: `name "range"`; berry: `name: range`. Names may be quoted.
            let dep = t.split([' ', ':']).next().unwrap_or("").trim_matches('"');
            if !dep.is_empty() {
                g.edges.entry(name.clone()).or_default().insert(dep.to_string());
            }
        }
    }
    g
}

/// pnpm (v5–v9): `importers:` or top-level dependency sections give roots;
/// `packages:`/`snapshots:` entries give edges.
pub(crate) fn graph_pnpm_lock(text: &str) -> Graph {
    let mut g = Graph::default();
    let mut section = String::new(); // top-level key
    let mut entry: Option<String> = None; // package / importer currently open
    let mut sub = String::new(); // dependencies / devDependencies / …
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    let name_of_key = |key: &str| -> Option<String> {
        let key = key.trim_end_matches(':').trim_matches('\'').trim_matches('"');
        let key = key.split('(').next().unwrap_or(key).trim_start_matches('/');
        let i = key.get(1..).and_then(|rest| rest.rfind(['@', '/'])).map(|i| i + 1)?;
        Some(key[..i].to_string())
    };
    let dep_name = |t: &str| t.split(':').next().unwrap_or("").trim().trim_matches('\'').trim_matches('"').to_string();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let ind = indent_of(line);
        let t = line.trim();
        if ind == 0 {
            section = t.trim_end_matches(':').to_string();
            entry = None;
            sub.clear();
            continue;
        }
        match section.as_str() {
            // Single-project v5/v6: top-level sections list roots directly.
            "dependencies" | "optionalDependencies" if ind == 2 => {
                g.roots.runtime.insert(dep_name(t));
            }
            "devDependencies" if ind == 2 => {
                g.roots.dev.insert(dep_name(t));
            }
            "importers" => match ind {
                2 => sub.clear(),
                4 => sub = t.trim_end_matches(':').to_string(),
                6 => match sub.as_str() {
                    "dependencies" | "optionalDependencies" => {
                        g.roots.runtime.insert(dep_name(t));
                    }
                    "devDependencies" => {
                        g.roots.dev.insert(dep_name(t));
                    }
                    _ => {}
                },
                _ => {}
            },
            "packages" | "snapshots" => match ind {
                2 => {
                    entry = name_of_key(t);
                    sub.clear();
                    if let Some(n) = &entry {
                        g.edges.entry(n.clone()).or_default();
                    }
                }
                4 => sub = t.trim_end_matches(':').to_string(),
                6 if matches!(sub.as_str(), "dependencies" | "optionalDependencies" | "peerDependencies") => {
                    if let Some(n) = &entry {
                        g.edges.entry(n.clone()).or_default().insert(dep_name(t));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    g
}

fn closure(edges: &BTreeMap<String, BTreeSet<String>>, roots: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = roots.into_iter().collect();
    while let Some(n) = stack.pop() {
        if seen.insert(n.clone()) {
            stack.extend(edges.get(&n).into_iter().flatten().filter(|d| !seen.contains(*d)).cloned());
        }
    }
    seen
}

/// Role per package name. `None` when no roots are known (no root entry in the
/// lockfile and no package.json): then the lockfile's own flags stand.
pub(crate) fn classify(graph: &Graph, extra_roots: Option<&Roots>) -> Option<BTreeMap<String, Role>> {
    let mut roots = graph.roots.clone();
    if let Some(r) = extra_roots {
        roots.runtime.extend(r.runtime.iter().cloned());
        roots.dev.extend(r.dev.iter().cloned());
    }
    if roots.runtime.is_empty() && roots.dev.is_empty() {
        return None;
    }
    let (build_roots, runtime_roots): (Vec<String>, Vec<String>) =
        roots.runtime.iter().cloned().partition(|n| is_build_tool(n));
    let runtime = closure(&graph.edges, runtime_roots);
    let build = closure(&graph.edges, build_roots);
    let dev = closure(&graph.edges, roots.dev.iter().cloned());
    let mut roles = BTreeMap::new();
    for n in dev {
        roles.insert(n, Role::Dev);
    }
    for n in build {
        roles.insert(n, Role::Build);
    }
    for n in runtime {
        roles.insert(n, Role::Runtime);
    }
    Some(roles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_tools_are_recognised_by_name_and_prefix() {
        for n in ["webpack", "@babel/core", "babel-loader", "eslint-plugin-vue", "@types/node", "vite", "jest"] {
            assert!(is_build_tool(n), "{n}");
        }
        for n in ["webpack-bundle-analyzer", "vite-plugin-pwa", "rollup-plugin-visualizer"] {
            assert!(is_build_tool(n), "{n}");
        }
        for n in ["vue", "axios", "lodash", "react", "swiper", "babelfish", "jestingly"] {
            assert!(!is_build_tool(n), "{n}: a prefix needs its separator");
        }
    }

    fn lock_v3() -> String {
        json!({"lockfileVersion": 3, "packages": {
            "": {"dependencies": {"vue": "^3", "webpack": "^5", "axios": "^1"}, "devDependencies": {"jest": "^29"}},
            "node_modules/vue": {"version": "3.4.0", "dependencies": {"@vue/shared": "3.4.0"}},
            "node_modules/@vue/shared": {"version": "3.4.0"},
            "node_modules/axios": {"version": "1.6.0", "dependencies": {"follow-redirects": "^1"}},
            "node_modules/follow-redirects": {"version": "1.15.0"},
            "node_modules/webpack": {"version": "5.0.0", "dependencies": {"loader-utils": "^2", "acorn": "^8"}},
            "node_modules/loader-utils": {"version": "2.0.0"},
            "node_modules/acorn": {"version": "8.0.0"},
            "node_modules/jest": {"version": "29.0.0", "dependencies": {"acorn": "^8", "micromatch": "^4"}},
            "node_modules/micromatch": {"version": "4.0.0"},
        }})
        .to_string()
    }

    #[test]
    fn webpack_under_dependencies_is_build_not_runtime() {
        let g = graph_package_lock(&lock_v3()).unwrap();
        let r = classify(&g, None).unwrap();
        assert_eq!(r["vue"], Role::Runtime);
        assert_eq!(r["@vue/shared"], Role::Runtime, "transitive runtime");
        assert_eq!(r["follow-redirects"], Role::Runtime);
        assert_eq!(r["webpack"], Role::Build, "declared under dependencies, still build tooling");
        assert_eq!(r["loader-utils"], Role::Build);
        assert_eq!(r["acorn"], Role::Build, "reachable from build and dev: build wins over dev");
        assert_eq!(r["micromatch"], Role::Dev);
    }

    #[test]
    fn runtime_wins_when_a_package_is_reachable_both_ways() {
        let mut g = graph_package_lock(&lock_v3()).unwrap();
        g.edges.get_mut("axios").unwrap().insert("acorn".into());
        assert_eq!(classify(&g, None).unwrap()["acorn"], Role::Runtime, "never under-count runtime");
    }

    #[test]
    fn yarn_v1_and_berry_edges_with_package_json_roots() {
        let v1 = "# yarn lockfile v1\n\n\"@scope/app-lib@^1.0.0\":\n  version \"1.0.0\"\n  dependencies:\n    lodash \"^4.17.0\"\n\nlodash@^4.17.0:\n  version \"4.17.21\"\n\nwebpack@^5:\n  version \"5.0.0\"\n  dependencies:\n    terser \"^5\"\n\nterser@^5:\n  version \"5.0.0\"\n";
        let g = graph_yarn_lock(v1);
        assert!(g.edges["@scope/app-lib"].contains("lodash"));
        assert!(g.edges["webpack"].contains("terser"));
        let pj = json!({"dependencies": {"@scope/app-lib": "^1", "webpack": "^5"}}).to_string();
        let r = classify(&g, roots_from_package_json(&pj).as_ref()).unwrap();
        assert_eq!(r["lodash"], Role::Runtime);
        assert_eq!(r["terser"], Role::Build);
        let berry = "\"vue@npm:^3\":\n  version: 3.4.0\n  resolution: \"vue@npm:3.4.0\"\n  dependencies:\n    \"@vue/shared\": 3.4.0\n";
        assert!(graph_yarn_lock(berry).edges["vue"].contains("@vue/shared"));
        assert!(classify(&graph_yarn_lock(v1), None).is_none(), "no roots known: lockfile flags stand");
    }

    #[test]
    fn pnpm_v9_importers_and_snapshots() {
        let text = "lockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    dependencies:\n      vue:\n        specifier: ^3\n        version: 3.4.0\n      vite:\n        specifier: ^5\n        version: 5.0.0\n    devDependencies:\n      vitest:\n        specifier: ^1\n        version: 1.0.0\n\npackages:\n\n  vue@3.4.0:\n    resolution: {integrity: x}\n\nsnapshots:\n\n  vue@3.4.0:\n    dependencies:\n      '@vue/shared': 3.4.0\n\n  '@vue/shared@3.4.0': {}\n\n  vite@5.0.0:\n    dependencies:\n      esbuild: 0.19.0\n      rollup: 4.0.0\n\n  vitest@1.0.0:\n    dependencies:\n      tinypool: 0.8.0\n";
        let g = graph_pnpm_lock(text);
        assert!(g.roots.runtime.contains("vue") && g.roots.runtime.contains("vite"));
        assert!(g.roots.dev.contains("vitest"));
        let r = classify(&g, None).unwrap();
        assert_eq!(r["@vue/shared"], Role::Runtime);
        assert_eq!(r["esbuild"], Role::Build);
        assert_eq!(r["tinypool"], Role::Dev);
    }

    #[test]
    fn pnpm_v6_single_project_top_level_sections() {
        let text = "lockfileVersion: '6.0'\n\ndependencies:\n  axios:\n    specifier: ^1\n    version: 1.6.0\n\ndevDependencies:\n  eslint:\n    specifier: ^8\n    version: 8.0.0\n\npackages:\n\n  /axios@1.6.0:\n    dependencies:\n      follow-redirects: 1.15.0\n    dev: false\n\n  /follow-redirects@1.15.0:\n    dev: false\n";
        let r = classify(&graph_pnpm_lock(text), None).unwrap();
        assert_eq!(r["follow-redirects"], Role::Runtime);
        assert_eq!(r["eslint"], Role::Dev);
    }
}
