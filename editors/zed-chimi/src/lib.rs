//! Zed support for Chimi: registers the language and launches `chimi-lsp`.

use zed_extension_api::settings::LspSettings;
use zed_extension_api::{self as zed, LanguageServerId, Result, Worktree, serde_json};

const SERVER: &str = "chimi-lsp";

struct ChimiExtension;

impl zed::Extension for ChimiExtension {
    fn new() -> Self {
        Self
    }

    /// Uses `lsp.chimi-lsp.binary` from settings when set, otherwise
    /// `chimi-lsp` from the worktree's shell `PATH`.
    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree(SERVER, worktree)
            .ok()
            .and_then(|settings| settings.binary);
        let configured_path = binary.as_ref().and_then(|binary| binary.path.clone());
        let command = configured_path
            .or_else(|| worktree.which(SERVER))
            .ok_or_else(|| {
                format!(
                    "{SERVER} was not found on PATH; build it with \
                     `cargo install --path compiler/chimi-lsp` or set lsp.{SERVER}.binary.path"
                )
            })?;
        let mut env = worktree.shell_env();
        if let Some(extra) = binary.as_ref().and_then(|binary| binary.env.clone()) {
            env.extend(extra);
        }
        Ok(zed::Command {
            command,
            args: binary.and_then(|binary| binary.arguments).unwrap_or_default(),
            env,
        })
    }

    /// Passes `lsp.chimi-lsp.initialization_options` through, which is where
    /// projects are configured: `{"projects": [{"root": ".", "entry": "app/main"}]}`.
    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<serde_json::Value>> {
        Ok(LspSettings::for_worktree(SERVER, worktree)
            .ok()
            .and_then(|settings| settings.initialization_options))
    }
}

zed::register_extension!(ChimiExtension);
