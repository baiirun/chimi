use std::process::ExitCode;

fn main() -> ExitCode {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    let result = chimi_lsp::server::run(&connection);
    drop(connection);
    let joined = io_threads.join();
    match result
        .map_err(|error| error.to_string())
        .and(joined.map_err(|error| error.to_string()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("chimi-lsp: {error}");
            ExitCode::FAILURE
        }
    }
}
