"""Mutants for CHI-319: the runner-supply table and how inspection and hover
report it. Each must be caught by tests/runner_needs.rs or the LSP hover
test."""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]
OPS = "src/operations.rs"
INS = "src/inspection.rs"
LSP = "../chimi-lsp/src/workspace.rs"


def table(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runner_needs"], filter="")


def hover(name, file, edits):
    return dict(name=name, file=file, edits=edits,
                command=["cargo", "test", "--release", "--quiet", "--manifest-path",
                         "../chimi-lsp/Cargo.toml", "--lib", "hover_says_where"],
                expect=["hover_says_where_a_runner_supplied_need_comes_from"])


MUTATIONS = [
    table("console takes a fixed name", OPS, [
        ("StandardType::Console => supply(None, TestSupply::CapturedConsole)",
         "StandardType::Console => supply(Some(\"console\"), TestSupply::CapturedConsole)"),
    ]),
    table("a test gets no random source", OPS, [
        ("supply(Some(\"random\"), TestSupply::SeededRandom)",
         "supply(Some(\"random\"), TestSupply::Using)"),
    ]),
    table("a test gets a file system", OPS, [
        ("StandardType::FileSystem => supply(None, TestSupply::Using)",
         "StandardType::FileSystem => supply(None, TestSupply::CapturedConsole)"),
    ]),
    table("runner providers are left out", OPS, [
        ("            .and_then(|provider| supply(Some(provider.need), TestSupply::Using)),",
         "            .and_then(|_| None),"),
    ]),
    table("every interface is host-supplied", OPS, [
        ("            .find(|provider| provider.interface == interface)\n            .and_then",
         "            .next()\n            .and_then"),
    ]),
    table("the need's name is ignored", INS, [
        (".is_none_or(|fixed| fixed == self.name)", ".is_none_or(|_| true)"),
    ]),
    table("a reference to a need reports nothing", INS, [
        ("        .filter(|symbol| typed.resolved.symbol(*symbol).kind == ProjectSymbolKind::Requirement)\n",
         "        .filter(|_| false)\n"),
    ]),
    hover("hover leaves out the need under the cursor", LSP, [
        ("        .chain(report.need())\n", ""),
    ]),
    hover("hover on a type name says nothing", LSP, [
        ("        let supply = target.runner()?;\n",
         "        let supply = target.runner()?;\n        if supply.fixed_name() != Some(\"\") {\n            return None;\n        }\n"),
    ]),
]
