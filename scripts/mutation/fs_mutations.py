"""Mutants of std/fs (CHI-248): the runner provider, the host core on both
backends, and the Chimi functions over it.

Run: scripts/mutation/mutate.py scripts/mutation/fs_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

OPERATIONS = "src/operations.rs"
HOST = "src/execution/host_operations.rs"
NATIVE = "../../runtime/chimi-runtime/src/intrinsics/host.rs"
FS = "std/fs.chimi"


def host(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["host_modules"])


MUTATIONS = [
    host("the runner closes another need name for files", OPERATIONS, [
        ('        need: "files",', '        need: "fs",')]),
    # The interpreter's host core.
    host("the interpreter reports NotADirectory as Other", HOST, [
        ("        ErrorKind::NotADirectory => -4,\n", "")]),
    host("the interpreter lets a NUL reach the system", HOST, [
        ("    if path.contains('\\0') {", "    if false {")]),
    host("the interpreter replaces without truncating", HOST, [
        ("0 => options.write(true).create(true).truncate(true),", "0 => options.write(true).create(true),")]),
    host("the interpreter ignores a handle's generation", HOST, [
        ("(i64::from(*current) == generation).then_some(file.as_mut()?)", "file.as_mut()")]),
    host("the interpreter lists in system order", HOST, [
        ("                    names.sort_unstable();\n", "")]),
    # The native host core (its table's rules are file_table_mutations.py's).
    host("native reports NotADirectory as Other", NATIVE, [
        ("        libc::ENOTDIR => NOT_A_DIRECTORY,\n", "")]),
    host("native replaces without truncating", NATIVE, [
        ("                        0 => libc::O_TRUNC,", "                        0 => 0,")]),
    host("native lets a NUL reach the system", NATIVE, [
        ("            if !libc::memchr(bytes.cast(), 0, length).is_null() {", "            if false {")]),
    host("native lists in system order", NATIVE, [
        ("                libc::qsort(names.0.as_mut_ptr().cast(), names.0.len(), size_of::<*mut c_char>(), Some(compare_names));\n",
         "")]),
    # std/fs in Chimi.
    host("read keeps only the last chunk", FS, [
        ("None -> break Bytes.concat(chunks)", "None -> break Bytes.concat(List.drop(chunks, List.length(chunks) - 1))")]),
    host("NotADirectory becomes NotFound", FS, [
        ("    -4 -> FsError.NotADirectory(path)", "    -4 -> FsError.NotFound(path)")]),
    host("create_directories does not create missing parents", FS, [
        ("      Some(parent) -> {\n        create_directories(parent)\n", "      Some(parent) -> {\n")]),
    host("an existing file counts as a directory", FS, [
        ("    Kind.Directory -> Void\n    _ -> Failure(FsError.AlreadyExists(path))", "    _ -> Void")]),
    host("files_within accepts a sibling with the same prefix", FS, [
        ("Text.starts_with(Path.to_text(normal), prefix)",
         "Text.starts_with(Path.to_text(normal), Path.to_text(inside))")]),
    host("files_within at the root adds a separator", FS, [
        ('    "/" -> "/"\n    text -> Text.append(text, "/")', '    text -> Text.append(text, "/")')]),
    host("the temporary directory is left behind", FS, [
        ("Resource.scoped(fn() { host_temporary_directory() }, remove_tree, use)",
         "Resource.scoped(fn() { host_temporary_directory() }, fn(path) { Void }, use)")]),
    host("removing the tree skips subdirectories", FS, [
        ("    1 -> List.each(list_names(path), fn(name) {", "    2 -> List.each(list_names(path), fn(name) {")]),
]
