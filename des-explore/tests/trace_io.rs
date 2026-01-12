use std::path::PathBuf;

use des_explore::io::{read_trace_from_path, write_trace_to_path, TraceFormat, TraceIoConfig};
use des_explore::trace::{DrawValue, RandomDraw, Trace, TraceEvent, TraceMeta};

fn temp_path(suffix: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let unique = format!(
        "des_explore_trace_io_{}_{}_{}{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("thread"),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        suffix
    );
    p.push(unique);
    p
}

fn sample_trace() -> Trace {
    let mut trace = Trace::new(TraceMeta {
        seed: 42,
        scenario: "roundtrip".to_string(),
    });

    trace.record(TraceEvent::RandomDraw(RandomDraw {
        time_nanos: Some(123),
        tag: "service".to_string(),
        site_id: 7,
        value: DrawValue::F64(1.25),
    }));

    trace
}

/// Roundtrip a trace through JSON encoding.
#[test]
fn json_roundtrip() {
    let trace = sample_trace();
    let path = temp_path(".json");

    write_trace_to_path(
        &path,
        &trace,
        TraceIoConfig {
            format: TraceFormat::Json,
        },
    )
    .unwrap();

    let read_back = read_trace_from_path(
        &path,
        TraceIoConfig {
            format: TraceFormat::Json,
        },
    )
    .unwrap();

    std::fs::remove_file(&path).ok();

    assert_eq!(read_back.meta.seed, trace.meta.seed);
    assert_eq!(read_back.meta.scenario, trace.meta.scenario);
    assert_eq!(read_back.events.len(), trace.events.len());
}

/// Roundtrip a trace through postcard encoding.
#[test]
fn postcard_roundtrip() {
    let trace = sample_trace();
    let path = temp_path(".bin");

    write_trace_to_path(
        &path,
        &trace,
        TraceIoConfig {
            format: TraceFormat::Postcard,
        },
    )
    .unwrap();

    let read_back = read_trace_from_path(
        &path,
        TraceIoConfig {
            format: TraceFormat::Postcard,
        },
    )
    .unwrap();

    std::fs::remove_file(&path).ok();

    assert_eq!(read_back.meta.seed, trace.meta.seed);
    assert_eq!(read_back.meta.scenario, trace.meta.scenario);
    assert_eq!(read_back.events.len(), trace.events.len());
}
