#![cfg(unix)]

mod support;

use std::{collections::BTreeMap, fs, sync::Arc};

use support::{
    CliContext, ResponseBody, TestServer, english_fixture, french_fixture, read_file,
    registry_with_urls, without_third_party_notice, write_registry,
};
use word_arena_lexicon::WordArenaPaths;
use word_arena_server::RuntimeLexicons;

#[tokio::test]
async fn clean_setup_is_idempotent_and_server_starts_offline() {
    let context = CliContext::new();
    let english = english_fixture();
    let french = french_fixture();
    let server = TestServer::start(BTreeMap::from([
        (
            "/en.tar.gz".to_owned(),
            ResponseBody::Complete(english.archive.clone()),
        ),
        (
            "/fr.tar.gz".to_owned(),
            ResponseBody::Complete(french.archive.clone()),
        ),
    ]));
    let registry = registry_with_urls(english, french, &server);
    write_registry(&context.registry, &registry);

    let first = context
        .command()
        .arg("setup")
        .output()
        .expect("first setup");
    assert!(first.status.success(), "{}", stderr(&first));
    assert_eq!(server.request_count(), 2);
    let second = context
        .command()
        .arg("setup")
        .output()
        .expect("second setup");
    assert!(second.status.success(), "{}", stderr(&second));
    assert_eq!(server.request_count(), 2, "second setup downloaded packs");

    drop(server);
    let offline = context
        .command()
        .args(["setup", "--offline"])
        .output()
        .expect("offline setup");
    assert!(offline.status.success(), "{}", stderr(&offline));
    let verify = context
        .command()
        .args(["lexicon", "verify"])
        .output()
        .expect("verify installed packs");
    assert!(verify.status.success(), "{}", stderr(&verify));

    let lexicons = Arc::new(
        RuntimeLexicons::load(&WordArenaPaths::from_base(context.data.clone()))
            .expect("server validates installed packs offline"),
    );
    assert_eq!(lexicons.english().word_count(), 2);
    assert_eq!(lexicons.french().word_count(), 2);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("offline server listener");
    let server_task = tokio::spawn(word_arena_server::serve(listener, lexicons));
    tokio::task::yield_now().await;
    assert!(
        !server_task.is_finished(),
        "offline server failed at startup"
    );
    server_task.abort();
}

#[test]
fn checksum_failure_prevents_partial_publication() {
    let context = CliContext::new();
    let english = english_fixture();
    let french = french_fixture();
    let server = TestServer::start(BTreeMap::from([
        (
            "/en.tar.gz".to_owned(),
            ResponseBody::Complete(english.archive.clone()),
        ),
        (
            "/fr.tar.gz".to_owned(),
            ResponseBody::Complete(french.archive.clone()),
        ),
    ]));
    let mut registry = registry_with_urls(english, french, &server);
    registry.packs[1].artifact_sha256 = "b".repeat(64);
    write_registry(&context.registry, &registry);

    let output = context.command().arg("setup").output().expect("bad setup");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("checksum mismatch"));
    assert!(
        !context
            .data
            .join("lexicons/word-arena-en-world-v1")
            .exists()
    );
    assert!(!context.data.join("lexicons/word-arena-fr-v1").exists());
}

#[test]
fn missing_notice_prevents_partial_publication() {
    let context = CliContext::new();
    let english = english_fixture();
    let french = without_third_party_notice(french_fixture());
    let server = TestServer::start(BTreeMap::from([
        (
            "/en.tar.gz".to_owned(),
            ResponseBody::Complete(english.archive.clone()),
        ),
        (
            "/fr.tar.gz".to_owned(),
            ResponseBody::Complete(french.archive.clone()),
        ),
    ]));
    let registry = registry_with_urls(english, french, &server);
    write_registry(&context.registry, &registry);

    let output = context
        .command()
        .arg("setup")
        .output()
        .expect("missing notice setup");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("THIRD_PARTY_NOTICES"));
    assert!(
        !context
            .data
            .join("lexicons/word-arena-en-world-v1")
            .exists()
    );
    assert!(!context.data.join("lexicons/word-arena-fr-v1").exists());
}

#[test]
fn interrupted_and_unavailable_downloads_preserve_installed_pack() {
    let context = CliContext::new();
    let english = english_fixture();
    let french = french_fixture();
    let good_server = TestServer::start(BTreeMap::from([
        (
            "/en.tar.gz".to_owned(),
            ResponseBody::Complete(english.archive.clone()),
        ),
        (
            "/fr.tar.gz".to_owned(),
            ResponseBody::Complete(french.archive.clone()),
        ),
    ]));
    let good_registry = registry_with_urls(english.clone(), french.clone(), &good_server);
    write_registry(&context.registry, &good_registry);
    let setup = context.command().arg("setup").output().expect("good setup");
    assert!(setup.status.success(), "{}", stderr(&setup));

    let english_path = installed_path(&context, &good_registry.packs[0]);
    let english_manifest = read_file(&english_path.join("manifest.toml"));
    let remove = context
        .command()
        .args(["lexicon", "remove", "word-arena-fr-v1"])
        .output()
        .expect("remove French fixture");
    assert!(remove.status.success(), "{}", stderr(&remove));
    fs::remove_file(
        context
            .data
            .join("cache/lexicons")
            .join(format!("{}.tar.gz", french.record.artifact_sha256)),
    )
    .expect("clear French fixture cache");
    drop(good_server);

    let truncated = french.archive[..french.archive.len() / 2].to_vec();
    let interrupted_server = TestServer::start(BTreeMap::from([(
        "/fr.tar.gz".to_owned(),
        ResponseBody::Interrupted {
            body: truncated,
            declared_length: french.archive.len(),
        },
    )]));
    let interrupted_registry =
        registry_with_urls(english.clone(), french.clone(), &interrupted_server);
    write_registry(&context.registry, &interrupted_registry);
    let interrupted = context
        .command()
        .arg("setup")
        .output()
        .expect("interrupted setup");
    assert!(!interrupted.status.success());
    let diagnostic = stderr(&interrupted);
    assert!(diagnostic.contains("existing installation was not changed"));
    assert!(diagnostic.contains("retry when the network is available"));
    assert_eq!(
        read_file(&english_path.join("manifest.toml")),
        english_manifest
    );
    assert!(!installed_path(&context, &interrupted_registry.packs[1]).exists());

    drop(interrupted_server);
    let unavailable = context
        .command()
        .arg("setup")
        .output()
        .expect("unavailable setup");
    assert!(!unavailable.status.success());
    let diagnostic = stderr(&unavailable);
    assert!(diagnostic.contains("retry when the network is available"));
    assert!(diagnostic.contains("--offline"));
    assert_eq!(
        read_file(&english_path.join("manifest.toml")),
        english_manifest
    );
}

#[test]
fn concurrent_setup_publishes_one_valid_identity_per_pack() {
    let context = CliContext::new();
    let english = english_fixture();
    let french = french_fixture();
    let server = TestServer::start(BTreeMap::from([
        (
            "/en.tar.gz".to_owned(),
            ResponseBody::Complete(english.archive.clone()),
        ),
        (
            "/fr.tar.gz".to_owned(),
            ResponseBody::Complete(french.archive.clone()),
        ),
    ]));
    let registry = registry_with_urls(english, french, &server);
    write_registry(&context.registry, &registry);

    let mut first = context.command();
    first.arg("setup");
    let mut second = context.command();
    second.arg("setup");
    let first = first.spawn().expect("first concurrent setup");
    let second = second.spawn().expect("second concurrent setup");
    let first = first.wait_with_output().expect("first setup result");
    let second = second.wait_with_output().expect("second setup result");
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(second.status.success(), "{}", stderr(&second));

    RuntimeLexicons::load(&WordArenaPaths::from_base(context.data.clone()))
        .expect("concurrently installed packs validate");
    for record in &registry.packs {
        let checksum_root = context
            .data
            .join("lexicons")
            .join(&record.pack_id)
            .join(&record.pack_version);
        let identities = fs::read_dir(checksum_root)
            .expect("installed version")
            .count();
        assert_eq!(identities, 1);
    }
}

fn installed_path(context: &CliContext, record: &xtask::PackRecord) -> std::path::PathBuf {
    context
        .data
        .join("lexicons")
        .join(&record.pack_id)
        .join(&record.pack_version)
        .join(&record.content_sha256)
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
