//! Tool utility tests (no network).

#[test]
fn test_language_detection() {
    use gl_mcp::tools::commits::detect_language;

    assert_eq!(detect_language("app/Main.kt"), "Kotlin");
    assert_eq!(detect_language("build.gradle.kts"), "Kotlin");
    assert_eq!(detect_language("App.java"), "Java");
    assert_eq!(detect_language("ViewController.swift"), "Swift");
    assert_eq!(detect_language("src/main.rs"), "Rust");
    assert_eq!(detect_language("index.ts"), "TypeScript");
    assert_eq!(detect_language("index.tsx"), "TypeScript");
    assert_eq!(detect_language("app.js"), "JavaScript");
    assert_eq!(detect_language("style.scss"), "CSS");
    assert_eq!(detect_language("playbook.yml"), "YAML/Ansible");
    assert_eq!(detect_language("handler.go"), "Go");
    assert_eq!(detect_language("auth.php"), "PHP");
    assert_eq!(detect_language("query.sql"), "SQL");
    assert_eq!(detect_language("app.vue"), "Vue");
    assert_eq!(detect_language("config.toml"), "TOML");
    assert_eq!(detect_language("Dockerfile"), "Docker");
    assert_eq!(detect_language("Makefile"), "Make");
    assert_eq!(detect_language(".github/workflows/ci.yml"), "YAML/Ansible"); // .yml extension matches first
    assert_eq!(detect_language(".gitlab-ci.yml"), "YAML/Ansible");
    assert_eq!(detect_language(".github/workflows/ci.conf"), "Config"); // .conf extension matches before path
    assert_eq!(detect_language("build.gradle"), "Gradle");
    assert_eq!(detect_language("script.py"), "Python");
    assert_eq!(detect_language("deploy.sh"), "Shell");
    assert_eq!(detect_language("README.md"), "Markdown");
    assert_eq!(detect_language("data.json"), "JSON");
    assert_eq!(detect_language("layout.xml"), "XML");
    assert_eq!(detect_language("template.html"), "HTML");
    assert_eq!(detect_language("random.xyz"), "Other");
    // Infrastructure
    assert_eq!(detect_language("roles/xray/templates/docker-compose.j2"), "Jinja2/Ansible");
    assert_eq!(detect_language("ansible/inventory/myapp/prod-node-bg25"), "Ansible/Inventory");
    assert_eq!(detect_language("ansible/scripts/myapp/proxy_node_in"), "Ansible/Inventory");
    assert_eq!(detect_language("main.tf"), "Terraform");
}

#[test]
fn test_write_tools_list() {
    use gl_mcp::tools::WRITE_TOOLS;

    assert!(WRITE_TOOLS.contains(&"create_issue"));
    assert!(WRITE_TOOLS.contains(&"update_issue"));
    assert!(WRITE_TOOLS.contains(&"add_note"));
    assert!(WRITE_TOOLS.contains(&"retry_pipeline"));
    assert!(WRITE_TOOLS.contains(&"cancel_pipeline"));
    assert!(!WRITE_TOOLS.contains(&"list_projects"));
    assert!(!WRITE_TOOLS.contains(&"get_commit_diff"));
}

#[test]
fn test_is_tool_enabled() {
    use gl_mcp::tools::is_tool_enabled;

    // Normal mode
    assert!(is_tool_enabled("list_projects", false, &[]));
    assert!(is_tool_enabled("create_issue", false, &[]));

    // Read-only mode
    assert!(is_tool_enabled("list_projects", true, &[]));
    assert!(!is_tool_enabled("create_issue", true, &[]));
    assert!(!is_tool_enabled("retry_pipeline", true, &[]));

    // Disabled tools
    let disabled = vec!["list_projects".to_string()];
    assert!(!is_tool_enabled("list_projects", false, &disabled));
    assert!(is_tool_enabled("get_project", false, &disabled));
}
