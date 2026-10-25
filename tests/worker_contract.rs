use clipvault::{
    config::{Config, Paths},
    engine::{Command, Engine, Event, SearchRequest},
    model::{Capture, SearchMode},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn keyword_search_works_without_cloud_or_a_desktop() {
    let d = tempfile::tempdir().unwrap();
    let paths = Paths::open(d.path().join("data")).unwrap();
    let engine = Engine::start(paths, Config::default(), Arc::new(|| {})).unwrap();
    assert!(engine.send(Command::Capture(
        Capture::Text("worker-contract needle".into()),
        "test".into()
    )));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(Event::Counts(all, _)) = engine.events.recv_timeout(Duration::from_millis(100))
            && all == 1
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "storage worker did not persist capture"
        );
    }
    assert!(engine.search(SearchRequest {
        generation: 42,
        query: "needle".into(),
        mode: SearchMode::Keyword
    }));
    loop {
        if let Ok(Event::Results {
            generation: 42,
            hits,
            ..
        }) = engine.events.recv_timeout(Duration::from_millis(100))
        {
            assert_eq!(hits.len(), 1);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "keyword search worker did not respond"
        );
    }
    drop(engine);
    // Workers wake at bounded intervals; no HTTP is enabled in this contract test.
    std::thread::sleep(Duration::from_millis(900));
}
