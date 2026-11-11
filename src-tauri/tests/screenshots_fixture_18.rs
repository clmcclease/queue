//! #18 (D-045): the webdriver screenshots seed `qa/webdriver/queue-18.json` as the app's
//! queue.json. It must load through the app's own reader, as the rows it's meant to show: a
//! Queued row with a notice, a Done row and a Failed row. Otherwise the CI capture would only
//! find out on a runner.
use queue_lib::queue::{Queue, Status};

#[test]
fn the_screenshot_fixture_loads_as_queued_done_and_failed_rows() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../qa/webdriver/queue-18.json"
    ))
    .unwrap();
    let queue = Queue::from_json(&bytes).expect("the fixture loads");
    let items = queue.items();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].status, Status::Queued);
    assert!(items[0].notice.is_some(), "the Queued row keeps its notice");
    assert!(
        matches!(&items[1].status, Status::Done { path } if path.ends_with("A finished video.mp4"))
    );
    assert_eq!(
        items[2].status,
        Status::Failed {
            reason: "Unsupported URL".into()
        }
    );
    assert_eq!(items[0].name.as_deref(), Some("A queued video"));
}
