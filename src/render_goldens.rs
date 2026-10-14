//! Golden snapshots of the read tools' renderers, fed synthetic GitLab responses.
//!
//! Each renderer was split from its async tool so the formatting, sorting, counting
//! and edge cases run without a network. Fixtures use placeholder hosts and names
//! only; secret-shaped values are assembled at runtime.

use serde_json::{json, Value};

use crate::golden::assert_golden;
use crate::tools::{commits, issues, pipelines, projects, repository};

fn ci_value() -> String {
    ["fixture", "-secret-", "value-", "0042"].concat()
}

#[test]
fn golden_repository_renderers() {
    let tree = vec![
        json!({"name": "src", "path": "src", "type": "tree"}),
        json!({"name": "main.rs", "path": "src/main.rs", "type": "blob"}),
        json!({"name": "README.md", "path": "README.md", "type": "blob"}),
    ];
    assert_golden("render/tree.md", &repository::render_get_tree("acme/api", "", true, 3, tree));

    let compare = json!({
        "commits": [
            {"short_id": "a1b2c3d4", "title": "feat: add parser", "author_name": "Ada"},
            {"short_id": "e5f6a7b8", "title": "fix: edge case", "author_name": "Lin"},
        ],
        "diffs": [
            {"new_path": "src/parser.rs", "new_file": true, "deleted_file": false, "diff": "+fn parse() {}\n+// done\n"},
            {"new_path": "old.txt", "new_file": false, "deleted_file": true, "diff": "-gone\n"},
            {"new_path": "src/lib.rs", "new_file": false, "deleted_file": false, "diff": "-a\n+b\n+c\n"},
        ],
    });
    assert_golden("render/compare.md", &repository::render_compare_branches("acme/api", "main", "feature/x", compare));

    let tags = vec![
        json!({"name": "v1.2.0", "message": "Release 1.2.0", "commit": {"short_id": "a1b2c3d4", "created_at": "2026-01-10T10:00:00Z"}}),
        json!({"name": "v1.1.0", "message": "", "commit": {"short_id": "e5f6a7b8", "created_at": "2025-12-01T10:00:00Z"}}),
    ];
    assert_golden("render/tags.md", &repository::render_list_tags(tags));

    let approvals = json!({"approved": false, "approvals_required": 2, "approvals_left": 1,
                           "approved_by": [{"user": {"username": "lin"}}]});
    assert_golden("render/mr_approvals.md", &repository::render_get_mr_approvals(7, approvals));

    let contributors = vec![
        json!({"name": "Ada", "email": "ada@example.com", "commits": 120}),
        json!({"name": "Lin", "email": "lin@example.com", "commits": 45}),
    ];
    assert_golden("render/contributors.md", &repository::render_get_contributors("acme/api", contributors));

    let rules = vec![
        json!({"name": "Code owners", "rule_type": "code_owner", "approvals_required": 1,
               "eligible_approvers": [{"username": "lin"}, {"username": "sam"}], "groups": []}),
        json!({"name": "Security", "rule_type": "regular", "approvals_required": 2,
               "eligible_approvers": [], "groups": [{"name": "security-team"}]}),
    ];
    assert_golden("render/approval_rules.md", &repository::render_get_approval_rules("acme/api", rules));

    let deployments = vec![
        json!({"created_at": "2026-01-15T10:00:00Z", "environment": {"name": "production"}, "user": {"username": "ada"}}),
        json!({"created_at": "2026-01-14T10:00:00Z", "environment": {"name": "production"}, "user": {"username": "lin"}}),
        json!({"created_at": "2026-01-02T10:00:00Z", "environment": {"name": "staging"}, "user": {"username": "ada"}}),
    ];
    assert_golden("render/deploy_frequency.md", &repository::render_get_deploy_frequency("acme/api", 30, deployments));
}

#[test]
fn golden_project_renderers() {
    let projects_list = vec![
        json!({"id": 1, "path_with_namespace": "acme/api", "description": "Core API", "visibility": "private", "star_count": 3}),
        json!({"id": 2, "path_with_namespace": "acme/web", "description": null, "visibility": "internal", "star_count": 0}),
    ];
    assert_golden("render/projects.md", &projects::render_list_projects(20, projects_list));

    let project = json!({"id": 1, "path_with_namespace": "acme/api", "description": "Core API", "visibility": "private",
        "default_branch": "main", "star_count": 3, "forks_count": 1, "open_issues_count": 4, "archived": false,
        "topics": ["rust", "api"], "created_at": "2025-01-01T00:00:00Z", "last_activity_at": "2026-01-15T00:00:00Z",
        "web_url": "https://git.example.com/acme/api"});
    assert_golden("render/project.md", &projects::render_get_project(project));

    let members = vec![
        json!({"username": "ada", "name": "Ada", "access_level": 50, "state": "active"}),
        json!({"username": "lin", "name": "Lin", "access_level": 30, "state": "blocked"}),
    ];
    assert_golden("render/members.md", &projects::render_list_members(members.clone()));
    assert_golden("render/group_members.md", &projects::render_get_group_members(members));

    let branches = vec![
        json!({"name": "main", "default": true, "protected": true,
               "commit": {"title": "Merge feature", "author_name": "Ada", "created_at": "2026-01-15T10:00:00Z"}}),
        json!({"name": "feature/x", "default": false, "protected": false,
               "commit": {"title": "wip", "author_name": "Lin", "created_at": "2025-11-01T10:00:00Z"}}),
    ];
    assert_golden("render/branches.md", &projects::render_list_branches(branches));

    let users = vec![
        json!({"username": "ada", "name": "Ada", "state": "active", "public_email": "ada@example.com", "is_admin": true}),
        json!({"username": "kim", "name": "Kim", "state": "blocked", "email": "kim@example.com"}),
    ];
    assert_golden("render/users.md", &projects::render_search_users(users));

    let events = vec![
        json!({"action_name": "pushed to", "author": {"username": "ada"}, "created_at": "2026-01-15T10:00:00Z",
               "push_data": {"commit_count": 3, "ref": "main", "ref_type": "branch"}}),
        json!({"action_name": "opened", "author": {"username": "lin"}, "created_at": "2026-01-15T09:00:00Z",
               "target_type": "MergeRequest", "target_title": "feat: parser"}),
        json!({"action_name": "commented on", "author": {"username": "sam"}, "created_at": "2026-01-14T09:00:00Z",
               "target_type": "Note", "target_title": "nit"}),
    ];
    assert_golden("render/events.md", &projects::render_get_project_events("acme/api", false, events.clone()));
    assert_golden("render/events_summary.txt", &projects::render_get_project_events("acme/api", true, events));

    let tokens = vec![
        json!({"name": "ci-read", "username": "gitlab+deploy-token-1", "scopes": ["read_repository"], "expires_at": "2026-06-01", "revoked": false}),
        json!({"name": "old", "username": "gitlab+deploy-token-2", "scopes": ["read_registry"], "expires_at": null, "revoked": true}),
    ];
    assert_golden("render/deploy_tokens.md", &projects::render_list_deploy_tokens(tokens));
}

#[test]
fn golden_issue_commit_pipeline_renderers() {
    let issues_list = vec![
        json!({"iid": 10, "title": "Crash on start", "state": "opened", "labels": ["bug", "P1"],
               "assignee": {"username": "ada"}, "references": {"full": "acme/api#10"}}),
        json!({"iid": 11, "title": "Docs", "state": "closed", "labels": [], "assignee": null,
               "references": {"full": "acme/web#11"}}),
    ];
    assert_golden("render/issues.md", &issues::render_search_issues(20, issues_list));
    let labels = vec![
        json!({"name": "bug", "color": "#d9534f", "description": "Something is broken", "open_issues_count": 4}),
        json!({"name": "docs", "color": "#5bc0de", "description": null, "open_issues_count": 0}),
    ];
    assert_golden("render/labels.md", &issues::render_list_labels(labels));
    let milestones = vec![
        json!({"title": "v1.3", "state": "active", "due_date": "2026-02-01", "description": "Next release"}),
        json!({"title": "v1.2", "state": "closed", "due_date": null, "description": ""}),
    ];
    assert_golden("render/milestones.md", &issues::render_get_milestones(milestones));

    let commits_list = vec![
        json!({"short_id": "a1b2c3d4", "title": "feat: parser", "author_name": "Ada", "author_email": "ada@example.com",
               "created_at": "2026-01-15T10:00:00Z", "committed_date": "2026-01-15T10:00:00Z"}),
        json!({"short_id": "e5f6a7b8", "title": "fix: crash", "author_name": "Lin", "author_email": "lin@example.com",
               "created_at": "2026-01-14T10:00:00Z", "committed_date": "2026-01-14T10:00:00Z"}),
    ];
    assert_golden("render/commits.md", &commits::render_list_commits("", false, commits_list.clone()));
    assert_golden("render/commits_summary.txt", &commits::render_list_commits("", true, commits_list));

    let pipelines_list = vec![
        json!({"id": 100, "status": "success", "ref": "main", "source": "push", "created_at": "2026-01-15T10:00:00Z",
               "web_url": "https://git.example.com/acme/api/-/pipelines/100", "sha": "a1b2c3d4e5f6"}),
        json!({"id": 101, "status": "failed", "ref": "feature/x", "source": "merge_request_event",
               "created_at": "2026-01-15T11:00:00Z", "web_url": "https://git.example.com/acme/api/-/pipelines/101", "sha": "0011223344"}),
    ];
    assert_golden("render/pipelines.md", &pipelines::render_list_pipelines("", pipelines_list.clone()));
    assert_golden("render/mr_pipelines.md", &pipelines::render_get_mr_pipelines(7, pipelines_list));

    let variables = vec![
        json!({"key": "DEPLOY_TOKEN", "value": ci_value(), "protected": true, "masked": true, "environment_scope": "*"}),
        json!({"key": "REGION", "value": "eu", "protected": false, "masked": false, "environment_scope": "production"}),
    ];
    let rendered = pipelines::render_get_ci_variables(variables);
    assert!(!rendered.contains(&ci_value()), "CI variable values are never shown");
    assert_golden("render/ci_variables.md", &rendered);
}

#[test]
fn empty_responses_render_a_message_not_an_empty_table() {
    let empty: Vec<Value> = Vec::new();
    for out in [
        repository::render_list_tags(empty.clone()),
        repository::render_get_contributors("p", empty.clone()),
        projects::render_list_members(empty.clone()),
        projects::render_list_branches(empty.clone()),
        issues::render_search_issues(20, empty.clone()),
        commits::render_list_commits("", false, empty.clone()),
        pipelines::render_list_pipelines("", empty.clone()),
        pipelines::render_get_ci_variables(empty),
    ] {
        assert!(!out.trim().is_empty(), "an empty response still says something");
    }
}
