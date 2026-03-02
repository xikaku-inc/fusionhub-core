use fusion_types::*;
use std::collections::VecDeque;
use std::time::{Duration, UNIX_EPOCH};

struct QueuedImuSample {
    imu_data: ImuData,
    queued_at: std::time::Instant,
}

#[test]
fn test_delay_queue_ordering() {
    let mut queue: VecDeque<QueuedImuSample> = VecDeque::new();

    let base_time = UNIX_EPOCH + Duration::from_secs(1);
    let now = std::time::Instant::now();

    for i in 0..10 {
        let imu = ImuData {
            timestamp: base_time + Duration::from_millis(i * 10),
            period: 0.01,
            gyroscope: Vec3d::new(0.0, 0.0, i as f64),
            accelerometer: Vec3d::new(0.0, 0.0, 1.0),
            ..Default::default()
        };
        queue.push_back(QueuedImuSample {
            imu_data: imu,
            queued_at: now + Duration::from_millis(i * 10),
        });
    }

    assert_eq!(queue.len(), 10);

    // Verify FIFO ordering: front should have the earliest timestamp
    let front = queue.front().unwrap();
    assert_eq!(front.imu_data.timestamp, base_time);

    let back = queue.back().unwrap();
    assert_eq!(
        back.imu_data.timestamp,
        base_time + Duration::from_millis(90)
    );

    // Verify sequential ordering by popping all elements
    let mut prev_timestamp = UNIX_EPOCH;
    while let Some(sample) = queue.pop_front() {
        assert!(
            sample.imu_data.timestamp >= prev_timestamp,
            "Samples should be ordered by timestamp"
        );
        prev_timestamp = sample.imu_data.timestamp;
    }
}

#[test]
fn test_delay_queue_trim() {
    let mut queue: VecDeque<QueuedImuSample> = VecDeque::new();

    let base_time = UNIX_EPOCH + Duration::from_secs(1);
    let now = std::time::Instant::now();

    // Fill queue with 20 samples
    for i in 0..20 {
        let imu = ImuData {
            timestamp: base_time + Duration::from_millis(i * 10),
            period: 0.01,
            gyroscope: Vec3d::new(0.0, 0.0, 0.0),
            accelerometer: Vec3d::new(0.0, 0.0, 1.0),
            ..Default::default()
        };
        queue.push_back(QueuedImuSample {
            imu_data: imu,
            queued_at: now + Duration::from_millis(i * 10),
        });
    }

    assert_eq!(queue.len(), 20);

    // Simulate processing: drain entries older than a delay threshold
    let delay = Duration::from_millis(100);
    let check_time = now + Duration::from_millis(150);
    let mut processed_count = 0;

    while let Some(front) = queue.front() {
        if check_time.duration_since(front.queued_at) >= delay {
            queue.pop_front();
            processed_count += 1;
        } else {
            break;
        }
    }

    // Samples queued at offsets 0..50ms should be drained (queued_at <= now+50ms,
    // and check_time - queued_at >= 100ms when queued_at <= now+50ms)
    assert!(
        processed_count > 0,
        "At least some old samples should have been drained"
    );
    assert!(
        queue.len() < 20,
        "Queue should be smaller after trimming old entries"
    );

    // Remaining entries should all be newer (queued more recently)
    for sample in &queue {
        let age = check_time.duration_since(sample.queued_at);
        assert!(
            age < delay,
            "Remaining samples should be newer than the delay threshold"
        );
    }
}

#[test]
fn test_delay_queue_empty() {
    let queue: VecDeque<QueuedImuSample> = VecDeque::new();
    assert!(queue.is_empty());
    assert_eq!(queue.len(), 0);
    assert!(queue.front().is_none());
}

#[test]
fn test_delay_queue_single_element() {
    let mut queue: VecDeque<QueuedImuSample> = VecDeque::new();

    let imu = ImuData {
        timestamp: UNIX_EPOCH + Duration::from_secs(1),
        period: 0.01,
        gyroscope: Vec3d::new(1.0, 2.0, 3.0),
        accelerometer: Vec3d::new(0.0, 0.0, 1.0),
        ..Default::default()
    };

    queue.push_back(QueuedImuSample {
        imu_data: imu,
        queued_at: std::time::Instant::now(),
    });

    assert_eq!(queue.len(), 1);

    let sample = queue.pop_front().unwrap();
    assert!((sample.imu_data.gyroscope.x - 1.0).abs() < 1e-15);
    assert!((sample.imu_data.gyroscope.y - 2.0).abs() < 1e-15);
    assert!((sample.imu_data.gyroscope.z - 3.0).abs() < 1e-15);

    assert!(queue.is_empty());
}

#[test]
fn test_delay_queue_preserves_imu_data() {
    let mut queue: VecDeque<QueuedImuSample> = VecDeque::new();

    let base_time = UNIX_EPOCH + Duration::from_secs(1);
    let gyro_values = [
        Vec3d::new(0.1, 0.0, 0.0),
        Vec3d::new(0.0, 0.2, 0.0),
        Vec3d::new(0.0, 0.0, 0.3),
    ];

    for (i, gyro) in gyro_values.iter().enumerate() {
        let imu = ImuData {
            timestamp: base_time + Duration::from_millis(i as u64 * 10),
            period: 0.01,
            gyroscope: *gyro,
            accelerometer: Vec3d::new(0.0, 0.0, 1.0),
            ..Default::default()
        };
        queue.push_back(QueuedImuSample {
            imu_data: imu,
            queued_at: std::time::Instant::now(),
        });
    }

    // Pop and verify data integrity
    let s0 = queue.pop_front().unwrap();
    assert!((s0.imu_data.gyroscope.x - 0.1).abs() < 1e-15);

    let s1 = queue.pop_front().unwrap();
    assert!((s1.imu_data.gyroscope.y - 0.2).abs() < 1e-15);

    let s2 = queue.pop_front().unwrap();
    assert!((s2.imu_data.gyroscope.z - 0.3).abs() < 1e-15);
}

#[test]
fn test_delay_queue_max_size_enforcement() {
    let mut queue: VecDeque<QueuedImuSample> = VecDeque::new();
    let max_size = 100;

    let base_time = UNIX_EPOCH + Duration::from_secs(1);
    let now = std::time::Instant::now();

    for i in 0..200 {
        let imu = ImuData {
            timestamp: base_time + Duration::from_millis(i),
            period: 0.001,
            ..Default::default()
        };
        queue.push_back(QueuedImuSample {
            imu_data: imu,
            queued_at: now + Duration::from_millis(i),
        });

        // Enforce max size by trimming the front
        while queue.len() > max_size {
            queue.pop_front();
        }
    }

    assert_eq!(queue.len(), max_size);

    // The oldest remaining sample should be from around index 100
    let oldest = queue.front().unwrap();
    assert_eq!(
        oldest.imu_data.timestamp,
        base_time + Duration::from_millis(100)
    );
}
