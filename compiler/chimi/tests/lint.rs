use chimi::Compiler;
use chimi::project::{LoadedProject, StandardTree};

#[test]
fn probe_std() {
    let tree = LoadedProject::standard_tree(&StandardTree::default()).unwrap();
    let report = Compiler::type_project(tree.sources());
    assert!(report.is_success());
    let findings = report.lint(true);
    eprint!("{}", tree.render_diagnostics(&findings));
}
