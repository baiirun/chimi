//! Drives the server over an in-memory connection, as an editor would.

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

fn scratch(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("chimi-lsp-protocol-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

fn receive(client: &Connection) -> Message {
    client
        .receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("server replies")
}

fn request(client: &Connection, id: i32, method: &str, params: Value) -> Value {
    client
        .sender
        .send(Request::new(RequestId::from(id), method.to_owned(), params).into())
        .unwrap();
    loop {
        if let Message::Response(response) = receive(client) {
            assert_eq!(response.id, RequestId::from(id));
            return response.response_result.expect("request succeeds");
        }
    }
}

fn notify(client: &Connection, method: &str, params: Value) {
    client
        .sender
        .send(Notification::new(method.to_owned(), params).into())
        .unwrap();
}

fn next_diagnostics(client: &Connection) -> Value {
    loop {
        if let Message::Notification(notification) = receive(client)
            && notification.method == "textDocument/publishDiagnostics"
        {
            return notification.params;
        }
    }
}

/// Guards the editor-visible flow: encoding negotiation, configured projects,
/// diagnostics that track unsaved edits, formatting, and clean shutdown.
#[test]
fn editor_session_publishes_diagnostics_and_formats() {
    let root = scratch("session");
    fs::write(root.join("app.chimi"), "pub fn main() -> Int { 0 }\n").unwrap();
    let uri = format!("file://{}", root.join("app.chimi").display());
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || chimi_lsp::server::run(&server).map_err(|e| e.to_string()));

    let initialized = request(
        &client,
        1,
        "initialize",
        json!({
            "capabilities": {"general": {"positionEncodings": ["utf-8", "utf-16"]}},
            "workspaceFolders": [{"uri": format!("file://{}", root.display()), "name": "fixture"}],
            "initializationOptions": {"projects": [{"root": ".", "entry": "app"}]},
        }),
    );
    assert_eq!(initialized["capabilities"]["positionEncoding"], "utf-8");
    notify(&client, "initialized", json!({}));

    notify(
        &client,
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "chimi", "version": 1,
            "text": "pub fn main() -> Int { missing() }\n"}}),
    );
    let published = next_diagnostics(&client);
    assert_eq!(published["uri"], uri);
    assert_eq!(published["version"], 1);
    let diagnostic = &published["diagnostics"][0];
    assert_eq!(diagnostic["source"], "chimi");
    assert_eq!(
        diagnostic["range"]["start"],
        json!({"line": 0, "character": 23})
    );

    notify(
        &client,
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": "pub fn main( ) -> Int {0}\n"}]}),
    );
    let published = next_diagnostics(&client);
    assert_eq!(published["version"], 2);
    assert_eq!(published["diagnostics"], json!([]));

    let edits = request(
        &client,
        2,
        "textDocument/formatting",
        json!({"textDocument": {"uri": uri}, "options": {"tabSize": 2, "insertSpaces": true}}),
    );
    assert_eq!(edits.as_array().map(Vec::len), Some(1));
    assert_eq!(edits[0]["range"]["end"], json!({"line": 1, "character": 0}));

    request(&client, 3, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    handle.join().unwrap().unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// Guards the wire shape of cursor queries: advertised capabilities, Markdown
/// hover contents, and a definition location whose URI names the target file.
#[test]
fn hover_and_definition_answer_over_the_protocol() {
    let root = scratch("queries");
    fs::write(
        root.join("app.chimi"),
        "import lib as Lib\npub fn main() -> Int { Lib.value() }\n",
    )
    .unwrap();
    fs::write(root.join("lib.chimi"), "pub fn value() -> Int { 1 }\n").unwrap();
    let uri = format!("file://{}", root.join("app.chimi").display());
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || chimi_lsp::server::run(&server).map_err(|e| e.to_string()));

    let initialized = request(
        &client,
        1,
        "initialize",
        json!({
            "capabilities": {"general": {"positionEncodings": ["utf-8"]}},
            "workspaceFolders": [{"uri": format!("file://{}", root.display()), "name": "fixture"}],
            "initializationOptions": {"projects": [{"root": ".", "entry": "app"}]},
        }),
    );
    assert_eq!(initialized["capabilities"]["hoverProvider"], true);
    assert_eq!(initialized["capabilities"]["definitionProvider"], true);
    notify(&client, "initialized", json!({}));
    notify(
        &client,
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "chimi", "version": 1,
            "text": "import lib as Lib\npub fn main() -> Int { Lib.value() }\n"}}),
    );
    next_diagnostics(&client);

    let position = json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": 28}});
    let hover = request(&client, 2, "textDocument/hover", position.clone());
    assert_eq!(hover["contents"]["kind"], "markdown");
    let definition = request(&client, 3, "textDocument/definition", position);
    assert_eq!(
        definition["uri"],
        format!("file://{}", root.join("lib.chimi").display())
    );
    assert_eq!(
        definition["range"]["start"],
        json!({"line": 0, "character": 7})
    );
    let nothing = request(
        &client,
        4,
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 0}}),
    );
    assert_eq!(nothing, Value::Null);

    request(&client, 5, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    handle.join().unwrap().unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn request_error(client: &Connection, id: i32, method: &str, params: Value) -> String {
    client
        .sender
        .send(Request::new(RequestId::from(id), method.to_owned(), params).into())
        .unwrap();
    loop {
        if let Message::Response(response) = receive(client) {
            assert_eq!(response.id, RequestId::from(id));
            return response.response_result.expect_err("request fails").message;
        }
    }
}

/// Guards the wire shape of references and rename: `Location` lists and a
/// `WorkspaceEdit` keyed by file URI, and a refused rename as an error reply.
#[test]
fn references_and_rename_answer_over_the_protocol() {
    let root = scratch("rename");
    let app_text = "import lib as Lib\npub fn main() -> Int { Lib.value() + Lib.value() }\n";
    fs::write(root.join("app.chimi"), app_text).unwrap();
    fs::write(root.join("lib.chimi"), "pub fn value() -> Int { 1 }\n").unwrap();
    let uri = format!("file://{}", root.join("app.chimi").display());
    let lib_uri = format!("file://{}", root.join("lib.chimi").display());
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || chimi_lsp::server::run(&server).map_err(|e| e.to_string()));

    let initialized = request(
        &client,
        1,
        "initialize",
        json!({
            "capabilities": {"general": {"positionEncodings": ["utf-8"]}},
            "workspaceFolders": [{"uri": format!("file://{}", root.display()), "name": "fixture"}],
            "initializationOptions": {"projects": [{"root": ".", "entry": "app"}]},
        }),
    );
    assert_eq!(initialized["capabilities"]["referencesProvider"], true);
    assert_eq!(initialized["capabilities"]["renameProvider"], true);
    notify(&client, "initialized", json!({}));
    notify(
        &client,
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "chimi", "version": 1, "text": app_text}}),
    );
    next_diagnostics(&client);

    let at_value = json!({"uri": uri});
    let references = request(
        &client,
        2,
        "textDocument/references",
        json!({"textDocument": at_value, "position": {"line": 1, "character": 28},
            "context": {"includeDeclaration": true}}),
    );
    let locations: Vec<(String, u64)> = references
        .as_array()
        .unwrap()
        .iter()
        .map(|location| {
            (
                location["uri"].as_str().unwrap().to_owned(),
                location["range"]["start"]["character"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        locations,
        vec![(lib_uri.clone(), 7), (uri.clone(), 27), (uri.clone(), 41)]
    );

    let edit = request(
        &client,
        3,
        "textDocument/rename",
        json!({"textDocument": at_value, "position": {"line": 1, "character": 28}, "newName": "amount"}),
    );
    assert_eq!(edit["changes"][&uri].as_array().map(Vec::len), Some(2));
    assert_eq!(edit["changes"][&lib_uri][0]["newText"], "amount");

    let refused = request_error(
        &client,
        4,
        "textDocument/rename",
        json!({"textDocument": at_value, "position": {"line": 1, "character": 28}, "newName": "Amount"}),
    );
    assert!(refused.contains("not a valid name"), "{refused}");

    request(&client, 5, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    handle.join().unwrap().unwrap();
    fs::remove_dir_all(root).unwrap();
}
