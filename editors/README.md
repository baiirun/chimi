# Editor support

- `tree-sitter-chimi/`: Tree-sitter grammar used for highlighting and outline.
  The compiler parser in `compiler/chimi/src/syntax.rs` stays the authority;
  this grammar treats line breaks as whitespace and is more permissive.
- `zed-chimi/`: Zed extension that registers Chimi and launches `chimi-lsp`.

## Changing the grammar

```sh
cd editors/tree-sitter-chimi
tree-sitter generate   # regenerates src/, which Zed builds and must be committed
tree-sitter test       # test/corpus
```

Highlight, bracket, indent, and outline queries live in
`zed-chimi/languages/chimi/` because they use Zed's capture names.

## Using it in Zed

1. Install the server: `cargo install --path compiler/chimi-lsp`.
2. Commit the grammar, then set `rev` in `zed-chimi/extension.toml` to that
   commit. Zed fetches the grammar by revision, so uncommitted changes are not
   visible to it. For local development set `repository` to
   `file:///<path to this checkout>`.
3. In Zed, run `zed: install dev extension` and choose `editors/zed-chimi`.
4. Configure projects, because Chimi has no manifest yet. In a project's
   `.zed/settings.json`:

   ```json
   {
     "lsp": {
       "chimi-lsp": {
         "initialization_options": {
           "projects": [{ "root": "programs/concurrency-pipeline", "entry": "app/main" }]
         }
       }
     }
   }
   ```

   Relative roots resolve against the first workspace folder. Files outside
   every configured project, or not imported by its entry, are checked alone.
   Changing these options requires restarting the language server.

Logs: `zed: open log`, or run `zed --foreground`.
