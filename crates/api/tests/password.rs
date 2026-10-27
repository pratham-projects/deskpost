use std::sync::Arc;

use api::config::{Argon2Cost, ConfigError};
use api::password::Hasher;

const CHEAP: Argon2Cost = Argon2Cost {
    memory_kib: 8,
    iterations: 1,
    parallelism: 1,
};

#[tokio::test]
async fn hashes_are_argon2id_salted_and_verify() {
    let h = Hasher::new(CHEAP, 2).unwrap();
    let (a, b) = (
        h.hash("hunter2hunter2".into()).await.unwrap(),
        h.hash("hunter2hunter2".into()).await.unwrap(),
    );
    assert!(a.starts_with("$argon2id$v=19$"), "{a}");
    assert_ne!(a, b, "fresh salt per hash");
    assert!(h.verify("hunter2hunter2".into(), Some(a.clone())).await);
    assert!(!h.verify("hunter2hunter3".into(), Some(a)).await);
}

#[tokio::test]
async fn production_cost_is_recorded_in_the_hash() {
    let h = Hasher::new(Argon2Cost::PRODUCTION, 1).unwrap();
    let phc = h.hash("a-long-enough-password".into()).await.unwrap();
    assert!(phc.contains("m=19456,t=2,p=1"), "{phc}");
}

#[tokio::test]
async fn unknown_accounts_and_corrupt_hashes_never_verify() {
    let h = Hasher::new(CHEAP, 1).unwrap();
    assert!(!h.verify("anything".into(), None).await);
    assert!(
        !h.verify("anything".into(), Some("not a phc string".into()))
            .await
    );
    assert!(!h.verify("anything".into(), Some(String::new())).await);
}

#[tokio::test]
async fn burst_beyond_the_semaphore_still_completes() {
    let h = Arc::new(Hasher::new(CHEAP, 2).unwrap());
    let phc = h.hash("pw-pw-pw-pw".into()).await.unwrap();
    let tasks: Vec<_> = (0..16)
        .map(|i| {
            let (h, phc) = (h.clone(), phc.clone());
            tokio::spawn(async move {
                h.verify(
                    if i % 2 == 0 {
                        "pw-pw-pw-pw"
                    } else {
                        "nope-nope"
                    }
                    .into(),
                    Some(phc),
                )
                .await
            })
        })
        .collect();
    for (i, t) in tasks.into_iter().enumerate() {
        assert_eq!(t.await.unwrap(), i % 2 == 0);
    }
}

#[test]
fn invalid_cost_is_a_config_error() {
    let bad = Argon2Cost {
        memory_kib: 1,
        iterations: 1,
        parallelism: 1,
    };
    let ConfigError(e) = Hasher::new(bad, 1)
        .err()
        .expect("memory below 8 KiB is rejected");
    assert!(e[0].contains("argon2"));
}
