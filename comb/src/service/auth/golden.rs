use super::*;
use crate::golden;

#[test]
fn registry_and_connection_bytes_preserve_credentials() {
    let (root, comb) = crate::disk::golden::seeded_comb(&[]);
    let registry = Registry::open(&comb.disk, comb.health.clone()).unwrap();
    let key = comb.disk.service_key().unwrap();
    let records = Records {
        credentials: vec![
            Record {
                id: golden::ID.parse().unwrap(),
                grant: Grant::Client {
                    name: "Writer".into(),
                    reviewer: false,
                },
                verifier: tag(&key, &"ab".repeat(32)).unwrap(),
            },
            Record {
                id: "23456789-1234-4234-8234-123456789abc".parse().unwrap(),
                grant: Grant::Bridge {
                    name: "Hive".into(),
                    reviewer_roles: vec!["reviewer".into()],
                },
                verifier: tag(&key, &"cd".repeat(32)).unwrap(),
            },
        ],
    };
    registry.persist(records).unwrap();
    golden::check(
        "credential-registry.json",
        std::fs::read(&registry.path).unwrap(),
    );
    let restored = Registry::open(&comb.disk, comb.health.clone()).unwrap();
    assert_eq!(restored.list().unwrap().len(), 2);
    let connection = Connection {
        format: hive_input::compatibility::Exact,
        socket: "/tmp/comb.sock".into(),
        credential: Credential {
            comb_id: (uuid::Uuid::parse_str(golden::ID).unwrap()).into(),
            id: golden::ID.parse().unwrap(),
            secret: "ab".repeat(32),
        },
    };
    let path = root.path().join("connection.json");
    connection.write(&path).unwrap();
    let authority = restored.authorize(&connection.credential, None).unwrap();
    assert_eq!(
        authority.actor.principal().unwrap(),
        Actor::Client {
            id: golden::ID.into(),
            name: "Writer".into()
        }
        .principal()
        .unwrap()
    );
    golden::check("connection.json", std::fs::read(&path).unwrap());
    assert_eq!(
        serde_json::to_vec(&Connection::read(&path).unwrap()).unwrap(),
        serde_json::to_vec(&connection).unwrap()
    );
}
