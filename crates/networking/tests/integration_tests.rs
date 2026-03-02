use std::time::Duration;

use fusion_types::{ImuData, StreamableData, Timestamp};
use networking::{DiskReader, DiskWriter, Publisher, Subscriber};

#[tokio::test]
async fn pub_sub_roundtrip() {
    let _ = env_logger::try_init();

    let publisher = Publisher::new("tcp://127.0.0.1:0");
    let endpoint = publisher.endpoint().to_owned();
    assert!(
        endpoint.starts_with("tcp://"),
        "Expected tcp:// endpoint, got: {}",
        endpoint
    );

    let (tx, rx) = std::sync::mpsc::channel();
    let subscriber = Subscriber::new(vec![endpoint]);
    subscriber
        .start_listening(move |data| {
            let _ = tx.send(data);
        })
        .unwrap();

    // Wait for ZMQ slow joiner
    tokio::time::sleep(Duration::from_millis(500)).await;

    let sent = StreamableData::Imu(ImuData {
        sender_id: "test".into(),
        ..Default::default()
    });
    publisher.publish(&sent).unwrap();

    let received = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("Subscriber did not receive the message");
    assert!(matches!(received, StreamableData::Imu(_)));
}

#[tokio::test]
async fn pub_sub_multiple_messages() {
    let _ = env_logger::try_init();

    let publisher = Publisher::new("tcp://127.0.0.1:0");
    let endpoint = publisher.endpoint().to_owned();

    let (tx, rx) = std::sync::mpsc::channel();
    let subscriber = Subscriber::new(vec![endpoint]);
    subscriber
        .start_listening(move |data| {
            let _ = tx.send(data);
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    for i in 0..5 {
        let data = StreamableData::Imu(ImuData {
            sender_id: format!("imu{}", i),
            ..Default::default()
        });
        publisher.publish(&data).unwrap();
    }

    let mut received = Vec::new();
    for _ in 0..5 {
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(data) => received.push(data),
            Err(_) => break,
        }
    }

    assert_eq!(received.len(), 5, "Expected 5 messages, got {}", received.len());
}

#[tokio::test]
async fn command_pub_sub_roundtrip() {
    let _ = env_logger::try_init();

    use fusion_types::ApiRequest;
    use networking::{CommandPublisher, CommandSubscriber};

    let cmd_pub = CommandPublisher::new("tcp://127.0.0.1:0");
    let endpoint = cmd_pub.endpoint().to_owned();

    let (tx, rx) = std::sync::mpsc::channel();
    let _cmd_sub = CommandSubscriber::new(
        move |req: &ApiRequest| {
            let _ = tx.send(req.clone());
        },
        vec![endpoint],
    );

    tokio::time::sleep(Duration::from_millis(500)).await;

    let request = ApiRequest {
        command: "configure".into(),
        topic: "test_node".into(),
        data: serde_json::json!({"key": "value"}),
        id: "req-001".into(),
    };
    cmd_pub.send(&request).unwrap();

    let received = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("CommandSubscriber did not receive the request");
    assert_eq!(received.command, "configure");
    assert_eq!(received.topic, "test_node");
}

#[test]
fn disk_writer_reader_roundtrip() {
    let _ = env_logger::try_init();

    let dir = std::env::temp_dir().join("fusionhub_test_disk_roundtrip.jsonl");
    let path = dir.to_str().unwrap().to_owned();

    let _ = std::fs::remove_file(&path);

    let writer = DiskWriter::new(&path);
    assert_eq!(writer.count(), 0);

    let ts1 = StreamableData::Timestamp(Timestamp::current());
    let ts2 = StreamableData::Timestamp(Timestamp::current());
    writer.write(&ts1).unwrap();
    writer.write(&ts2).unwrap();
    assert_eq!(writer.count(), 2);

    drop(writer);

    let reader = DiskReader::new(&path);
    let mut items = Vec::new();
    reader.read(|data| items.push(data)).unwrap();
    assert_eq!(items.len(), 2);
    assert!(matches!(items[0], StreamableData::Timestamp(_)));
    assert!(matches!(items[1], StreamableData::Timestamp(_)));

    let _ = std::fs::remove_file(&path);
}

/// inproc:// publisher returns the inproc name (no TCP rewrite).
#[test]
fn resolve_inproc_endpoint() {
    let publisher = Publisher::new("inproc://test_resolve_ep");
    assert_eq!(
        publisher.endpoint(),
        "inproc://test_resolve_ep",
        "inproc publisher should return original inproc:// name"
    );
}

/// Subscriber with a mix of reachable and unreachable TCP endpoints.
#[tokio::test]
async fn pub_sub_unreachable_endpoint_does_not_block() {
    let _ = env_logger::try_init();

    let publisher = Publisher::new("tcp://127.0.0.1:0");
    let good_endpoint = publisher.endpoint().to_owned();

    let bad_endpoints = vec![
        "tcp://127.0.0.1:1".to_owned(),
        "tcp://127.0.0.1:19".to_owned(),
    ];

    let mut endpoints = bad_endpoints;
    endpoints.push(good_endpoint);

    let (tx, rx) = std::sync::mpsc::channel();
    let subscriber = Subscriber::new(endpoints);
    subscriber
        .start_listening(move |data| {
            let _ = tx.send(data);
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    for i in 0..10 {
        let data = StreamableData::Imu(ImuData {
            sender_id: format!("sensor{}", i),
            ..Default::default()
        });
        publisher.publish(&data).unwrap();
    }

    let mut received = Vec::new();
    for _ in 0..10 {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(data) => received.push(data),
            Err(_) => break,
        }
    }

    assert_eq!(
        received.len(),
        10,
        "Expected 10 messages despite unreachable endpoints, got {}",
        received.len()
    );
}

/// Multiple publishers, one subscriber connected to all of them.
#[tokio::test]
async fn pub_sub_multiple_publishers() {
    let _ = env_logger::try_init();

    let pub_a = Publisher::new("tcp://127.0.0.1:0");
    let pub_b = Publisher::new("tcp://127.0.0.1:0");

    let endpoints = vec![
        pub_a.endpoint().to_owned(),
        pub_b.endpoint().to_owned(),
    ];

    let (tx, rx) = std::sync::mpsc::channel();
    let subscriber = Subscriber::new(endpoints);
    subscriber
        .start_listening(move |data| {
            let _ = tx.send(data);
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    for i in 0..3 {
        let data = StreamableData::Imu(ImuData {
            sender_id: format!("A{}", i),
            ..Default::default()
        });
        pub_a.publish(&data).unwrap();
    }

    for i in 0..3 {
        let data = StreamableData::Imu(ImuData {
            sender_id: format!("B{}", i),
            ..Default::default()
        });
        pub_b.publish(&data).unwrap();
    }

    let mut received = Vec::new();
    for _ in 0..6 {
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(data) => received.push(data),
            Err(_) => break,
        }
    }

    assert_eq!(
        received.len(),
        6,
        "Expected 6 messages from two publishers, got {}",
        received.len()
    );

    let sender_ids: Vec<String> = received
        .iter()
        .filter_map(|d| d.sender_id().map(|s| s.to_owned()))
        .collect();
    assert!(
        sender_ids.iter().any(|id| id.starts_with('A')),
        "Missing messages from publisher A"
    );
    assert!(
        sender_ids.iter().any(|id| id.starts_with('B')),
        "Missing messages from publisher B"
    );
}

/// Inproc roundtrip via broadcast channel — no ZMQ, no slow-joiner delay.
#[tokio::test]
async fn pub_sub_inproc_roundtrip() {
    let _ = env_logger::try_init();

    let publisher = Publisher::new("inproc://test_inproc_rt");
    assert_eq!(publisher.endpoint(), "inproc://test_inproc_rt");

    let (tx, rx) = std::sync::mpsc::channel();
    let subscriber = Subscriber::new(vec!["inproc://test_inproc_rt".into()]);
    subscriber
        .start_listening(move |data| {
            let _ = tx.send(data);
        })
        .unwrap();

    // Brief yield to let the subscriber task start — no 500ms slow-joiner needed.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let sent = StreamableData::Imu(ImuData {
        sender_id: "inproc_test".into(),
        ..Default::default()
    });
    publisher.publish(&sent).unwrap();

    let received = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("Subscriber did not receive message via inproc broadcast");
    assert_eq!(received.sender_id(), Some("inproc_test"));
}

/// High-throughput test: 1000 messages at burst rate via TCP.
#[tokio::test]
async fn pub_sub_high_throughput() {
    let _ = env_logger::try_init();

    let publisher = Publisher::new("tcp://127.0.0.1:0");
    let endpoint = publisher.endpoint().to_owned();

    let (tx, rx) = std::sync::mpsc::channel();
    let subscriber = Subscriber::new(vec![endpoint]);
    subscriber
        .start_listening(move |data| {
            let _ = tx.send(data);
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    let count = 1000;
    for i in 0..count {
        let data = StreamableData::Imu(ImuData {
            sender_id: format!("burst{}", i),
            ..Default::default()
        });
        publisher.publish(&data).unwrap();
    }

    let mut received = 0usize;
    loop {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(_) => received += 1,
            Err(_) => break,
        }
        if received >= count {
            break;
        }
    }

    assert_eq!(
        received, count,
        "Expected {} messages, received {}",
        count, received
    );
}

/// Subscriber is dropped while messages are in flight.
#[tokio::test]
async fn pub_sub_subscriber_drop_while_active() {
    let _ = env_logger::try_init();

    let publisher = Publisher::new("tcp://127.0.0.1:0");
    let endpoint = publisher.endpoint().to_owned();

    let (tx, rx) = std::sync::mpsc::channel();

    {
        let subscriber = Subscriber::new(vec![endpoint]);
        subscriber
            .start_listening(move |data| {
                let _ = tx.send(data);
            })
            .unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        let data = StreamableData::Imu(ImuData {
            sender_id: "before_drop".into(),
            ..Default::default()
        });
        publisher.publish(&data).unwrap();

        let _ = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("Should receive at least one message before drop");
    }

    let data = StreamableData::Imu(ImuData {
        sender_id: "after_drop".into(),
        ..Default::default()
    });
    let _ = publisher.publish(&data);

    tokio::time::sleep(Duration::from_millis(100)).await;
}

// --- Broadcast-specific tests ---

/// Inproc command pub/sub roundtrip via broadcast — no JSON serialization.
#[tokio::test]
async fn inproc_command_pub_sub_roundtrip() {
    let _ = env_logger::try_init();

    use fusion_types::ApiRequest;
    use networking::{CommandPublisher, CommandSubscriber};

    let cmd_pub = CommandPublisher::new("inproc://test_cmd_bc");
    assert_eq!(cmd_pub.endpoint(), "inproc://test_cmd_bc");

    let (tx, rx) = std::sync::mpsc::channel();
    let _cmd_sub = CommandSubscriber::new(
        move |req: &ApiRequest| {
            let _ = tx.send(req.clone());
        },
        vec!["inproc://test_cmd_bc".into()],
    );

    tokio::time::sleep(Duration::from_millis(50)).await;

    let request = ApiRequest {
        command: "reload".into(),
        topic: "sensor".into(),
        data: serde_json::json!({"rate": 100}),
        id: "cmd-bc-001".into(),
    };
    cmd_pub.send(&request).unwrap();

    let received = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("CommandSubscriber did not receive via broadcast");
    assert_eq!(received.command, "reload");
    assert_eq!(received.id, "cmd-bc-001");
}

/// One inproc publisher, two subscribers — both receive all messages.
#[tokio::test]
async fn inproc_multiple_subscribers() {
    let _ = env_logger::try_init();

    let publisher = Publisher::new("inproc://test_multi_sub");

    let (tx1, rx1) = std::sync::mpsc::channel();
    let sub1 = Subscriber::new(vec!["inproc://test_multi_sub".into()]);
    sub1.start_listening(move |data| {
        let _ = tx1.send(data);
    })
    .unwrap();

    let (tx2, rx2) = std::sync::mpsc::channel();
    let sub2 = Subscriber::new(vec!["inproc://test_multi_sub".into()]);
    sub2.start_listening(move |data| {
        let _ = tx2.send(data);
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    for i in 0..5 {
        let data = StreamableData::Imu(ImuData {
            sender_id: format!("multi{}", i),
            ..Default::default()
        });
        publisher.publish(&data).unwrap();
    }

    let mut count1 = 0;
    for _ in 0..5 {
        if rx1.recv_timeout(Duration::from_secs(1)).is_ok() {
            count1 += 1;
        }
    }

    let mut count2 = 0;
    for _ in 0..5 {
        if rx2.recv_timeout(Duration::from_secs(1)).is_ok() {
            count2 += 1;
        }
    }

    assert_eq!(count1, 5, "Subscriber 1 expected 5, got {}", count1);
    assert_eq!(count2, 5, "Subscriber 2 expected 5, got {}", count2);

    drop(sub1);
    drop(sub2);
}

/// Mixed subscriber: one inproc endpoint + one TCP endpoint in the same subscriber.
#[tokio::test]
async fn mixed_inproc_tcp_subscriber() {
    let _ = env_logger::try_init();

    let inproc_pub = Publisher::new("inproc://test_mixed_src");
    let tcp_pub = Publisher::new("tcp://127.0.0.1:0");
    let tcp_ep = tcp_pub.endpoint().to_owned();

    let endpoints = vec![
        "inproc://test_mixed_src".into(),
        tcp_ep,
    ];

    let (tx, rx) = std::sync::mpsc::channel();
    let subscriber = Subscriber::new(endpoints);
    subscriber
        .start_listening(move |data| {
            let _ = tx.send(data);
        })
        .unwrap();

    // Need 500ms for the TCP slow-joiner
    tokio::time::sleep(Duration::from_millis(500)).await;

    let data_inproc = StreamableData::Imu(ImuData {
        sender_id: "from_inproc".into(),
        ..Default::default()
    });
    inproc_pub.publish(&data_inproc).unwrap();

    let data_tcp = StreamableData::Imu(ImuData {
        sender_id: "from_tcp".into(),
        ..Default::default()
    });
    tcp_pub.publish(&data_tcp).unwrap();

    let mut received = Vec::new();
    for _ in 0..2 {
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(data) => received.push(data),
            Err(_) => break,
        }
    }

    assert_eq!(received.len(), 2, "Expected 2 messages (1 inproc + 1 tcp), got {}", received.len());

    let sender_ids: Vec<String> = received
        .iter()
        .filter_map(|d| d.sender_id().map(|s| s.to_owned()))
        .collect();
    assert!(sender_ids.contains(&"from_inproc".to_owned()), "Missing inproc message");
    assert!(sender_ids.contains(&"from_tcp".to_owned()), "Missing tcp message");
}

/// Dropping an inproc publisher closes the broadcast channel cleanly.
#[tokio::test]
async fn inproc_publisher_drop_closes_subscribers() {
    let _ = env_logger::try_init();

    let publisher = Publisher::new("inproc://test_drop_close");

    let (tx, rx) = std::sync::mpsc::channel();
    let _subscriber = Subscriber::new(vec!["inproc://test_drop_close".into()]);
    _subscriber
        .start_listening(move |data| {
            let _ = tx.send(data);
        })
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let data = StreamableData::Imu(ImuData {
        sender_id: "pre_drop".into(),
        ..Default::default()
    });
    publisher.publish(&data).unwrap();

    let _ = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("Should receive message before publisher drop");

    // Drop publisher — subscriber should see Closed and exit cleanly
    drop(publisher);
    tokio::time::sleep(Duration::from_millis(100)).await;
    // No panic = success
}
