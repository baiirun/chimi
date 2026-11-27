"""Mutants of `chimi build --target` (Phase 9, T8; CHI-280): which targets
build, what a refused one says, and how another target is built: its code
generator, its C tools, its runtime pack, its link and its build report.

The cross_build mutants need a compiler with runtime packs, and the
missing-pack mutant one without x86_64-apple-darwin's, so run with the
Linux CI job's CHIMI_CROSS_TARGETS (arm64 Linux and arm64 macOS) and Zig on
PATH, on an x86_64 Linux host (tooling.md, Cross-compilation).

Run: CHIMI_CROSS_TARGETS=aarch64-unknown-linux-gnu,aarch64-apple-darwin scripts/mutation/mutate.py scripts/mutation/build_target_mutations.py
Results: scripts/mutation/build_target_results.txt.
"""

CRATE = "compiler/chimi"

N = "src/backend/native.rs"
T = "src/backend/target.rs"
X = "src/backend/cross_targets.rs"
M = "src/main.rs"


def mutant(name, file, edits, targets=("build_target",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


def cross(name, file, edits):
    return mutant(name, file, edits, targets=("cross_build",))


MUTATIONS = [
    mutant("the host is refused", T, [(
        "        if same_platform(&wanted, &host_triple) {",
        "        if false {",
    )]),
    mutant("any target is the host", T, [(
        "        if same_platform(&wanted, &host_triple) {",
        "        if true {",
    )]),
    mutant("the target is not checked", M, [(
        "    let target = match target.map_or_else(BuildTarget::host, BuildTarget::resolve) {",
        "    let target = match target.map_or_else(BuildTarget::host, |_| BuildTarget::host()) {",
    )]),
    mutant("the vendor must match", T, [(
        "    left.architecture == right.architecture\n",
        "    left.vendor == right.vendor && left.architecture == right.architecture\n",
    )], targets=("build_target", "cross_build")),
    mutant("the environment is ignored", T, [(
        "        && left.environment == right.environment\n",
        "",
    )]),
    mutant("a missing pack is not reported", T, [(
        "        let Some(pack) = embedded_pack(target.triple) else {",
        "        let Some(pack) = embedded_pack(target.triple).or_else(|| embedded_pack(\"x86_64-apple-darwin\")).or_else(|| embedded_pack(\"aarch64-apple-darwin\")) else {",
    )]),
    cross("any embedded pack serves every target", T, [(
        "        .find(|(name, _, _)| *name == triple)",
        "        .find(|_| true)",
    )]),
    cross("another target gets the host's code generator", T, [(
        "            isa: cross_isa(target.triple)?,",
        "            isa: host_isa()?,",
    )]),
    cross("another target builds its C with the host's tools", N, [(
        "    let tools = target.c_tools();",
        "    let tools = CTools::host();",
    )]),
    cross("no cross target is macOS", T, [(
        "            Toolchain::Cross { target, .. } => target.macos(),",
        "            Toolchain::Cross { .. } => false,",
    )]),
    cross("Linux targets link no unwinder", N, [(
        "    for library in target.link_libraries() {",
        "    for library in target.link_libraries().iter().take(0) {",
    )]),
    cross("the report names the host's platform", T, [(
        "            Toolchain::Cross { target, .. } => (target.os, target.arch),",
        "            Toolchain::Cross { .. } => (std::env::consts::OS, std::env::consts::ARCH),",
    )]),
    cross("an ELF file's dependencies are not read", N, [(
        "                    if entry.tag(endian) == i64::from(elf::DT_NEEDED) {",
        "                    if entry.tag(endian) != i64::from(elf::DT_NEEDED) {",
    )]),
    cross("a Mach-O file's dependencies are not read", N, [(
        "                if let Some(dylib) = command.dylib().map_err(malformed)? {",
        "                if let Some(dylib) = command.dylib().map_err(malformed)?.filter(|_| false) {",
    )]),
    cross("a cross build reads dependencies with the host's tools", N, [(
        "    let dynamic_dependencies = if target.is_host() {",
        "    let dynamic_dependencies = if true {",
    )]),
    cross("Zig is not checked", T, [(
        "        let zig_version = zig_version(&zig).map_err(|problem| {",
        "        let zig_version = zig_version(&zig).or_else(|_| Ok::<_, String>(String::new())).map_err(|problem: String| {",
    )]),
    mutant("the script passes `-?` to Zig", X, [(
        "         \\x20   '-?') exit 1 ;;\\n\\\n",
        "",
    )], targets=("lib",), filter="backend::target"),
    mutant("the script keeps Clang's target flag", X, [(
        "         \\x20   --target=*|-mmacosx-version-min=*) ;;\\n\\\n",
        "         \\x20   -mmacosx-version-min=*) ;;\\n\\\n",
    )], targets=("lib",), filter="backend::target"),
]
