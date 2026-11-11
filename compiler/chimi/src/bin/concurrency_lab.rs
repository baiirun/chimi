use chimi::concurrency_lab::{
    run_memory_probe, run_observability, run_performance, run_probe, run_runtime, run_smoke,
    run_static,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("concurrency conformance failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args();
    let _program = arguments.next();
    let kind = arguments.next().ok_or_else(|| {
        "expected static, runtime, observability, performance, smoke, or all".to_owned()
    })?;
    if arguments.next().is_some() {
        return Err("concurrency conformance accepts exactly one evidence kind".into());
    }
    if kind == "probe" {
        return run_probe();
    }
    if kind == "memory-probe" {
        let (parked, retained) = run_memory_probe()?;
        println!("{parked} {retained}");
        return Ok(());
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve conformance executable: {error}"))?;
    let reports = match kind.as_str() {
        "static" => vec![run_static()?],
        "runtime" => vec![run_runtime()?],
        "observability" => vec![run_observability()?],
        "performance" => vec![run_performance(&executable)?],
        "smoke" => vec![run_smoke()?],
        "all" => vec![
            run_static()?,
            run_runtime()?,
            run_observability()?,
            run_performance(&executable)?,
            run_smoke()?,
        ],
        _ => return Err(format!("unknown evidence kind `{kind}`")),
    };
    for report in reports {
        print!("{}", report.render_json());
    }
    Ok(())
}
