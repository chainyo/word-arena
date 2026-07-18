use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use flate2::{Compression, GzBuilder};
use fst::SetBuilder;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use word_arena_lexicon::{
    BuilderDescriptor, FileDescriptor, NormalizationDescriptor, PackManifest, PolicyDescriptor,
    SourceDescriptor, calculate_content_sha256,
};
use xtask::{PackRecord, PackRegistry};

#[derive(Clone, Debug)]
pub struct FixtureArtifact {
    pub record: PackRecord,
    pub archive: Vec<u8>,
}

pub fn fixture_artifact(
    pack_id: &str,
    locale: &str,
    profile: &str,
    license_id: &str,
    words: &[&str],
) -> FixtureArtifact {
    let workspace = TempDir::new().expect("fixture workspace");
    let root = workspace.path();
    fs::create_dir(root.join("curation")).expect("curation directory");
    fs::write(root.join("LICENSE"), format!("fixture {license_id}\n")).expect("license");
    fs::write(root.join("SOURCE.md"), "# Fixture source\n").expect("source notice");
    fs::write(
        root.join("THIRD_PARTY_NOTICES"),
        "Fixture third-party notices\n",
    )
    .expect("third-party notice");
    fs::write(
        root.join("curation/additions.toml"),
        "schema_version = 1\noverrides = []\n",
    )
    .expect("additions");
    fs::write(
        root.join("curation/removals.toml"),
        "schema_version = 1\noverrides = []\n",
    )
    .expect("removals");
    let mut builder = SetBuilder::memory();
    for word in words {
        builder.insert(word).expect("sorted fixture key");
    }
    fs::write(
        root.join("lexicon.fst"),
        builder.into_inner().expect("fixture FST"),
    )
    .expect("runtime index");

    let mut files = [
        "LICENSE",
        "SOURCE.md",
        "THIRD_PARTY_NOTICES",
        "curation/additions.toml",
        "curation/removals.toml",
        "lexicon.fst",
    ]
    .into_iter()
    .map(|relative| describe_file(root, relative))
    .collect::<Vec<_>>();
    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    let mut manifest = PackManifest {
        format_version: 1,
        pack_id: pack_id.to_owned(),
        pack_version: "1.0.0".to_owned(),
        locale: locale.to_owned(),
        word_count: u64::try_from(words.len()).expect("fixture count"),
        content_sha256: "0".repeat(64),
        normalization: NormalizationDescriptor {
            algorithm: "word-arena-board-key".to_owned(),
            version: 1,
            profile: profile.to_owned(),
        },
        source: SourceDescriptor {
            id: format!("fixture-{locale}"),
            revision: "1".to_owned(),
            archive_sha256: "a".repeat(64),
            license_id: license_id.to_owned(),
        },
        policy: PolicyDescriptor {
            id: format!("fixture-{locale}-policy"),
            version: 1,
        },
        builder: BuilderDescriptor {
            name: "fixture-builder".to_owned(),
            version: "1.0.0".to_owned(),
        },
        files,
    };
    manifest.content_sha256 =
        calculate_content_sha256(root, &manifest.files).expect("fixture content hash");
    let encoded = toml::to_string_pretty(&manifest).expect("fixture manifest TOML");
    fs::write(root.join("manifest.toml"), encoded).expect("fixture manifest");
    let archive = archive_directory(root);
    let archive_sha256 = sha256_bytes(&archive);
    FixtureArtifact {
        record: PackRecord {
            pack_id: pack_id.to_owned(),
            pack_version: "1.0.0".to_owned(),
            format_version: 1,
            locale: locale.to_owned(),
            normalization_algorithm: "word-arena-board-key".to_owned(),
            normalization_version: 1,
            normalization_profile: profile.to_owned(),
            content_sha256: manifest.content_sha256,
            artifact_url: "https://example.invalid/fixture.tar.gz".to_owned(),
            artifact_size_bytes: u64::try_from(archive.len()).expect("fixture archive size"),
            artifact_sha256: archive_sha256,
            license_id: license_id.to_owned(),
        },
        archive,
    }
}

pub fn english_fixture() -> FixtureArtifact {
    fixture_artifact(
        "word-arena-en-world-v1",
        "en",
        "en-basic-latin-v1",
        "LicenseRef-SCOWL-v1",
        &["CAT", "DOG"],
    )
}

pub fn french_fixture() -> FixtureArtifact {
    fixture_artifact(
        "word-arena-fr-v1",
        "fr",
        "fr-basic-latin-fold-v1",
        "LicenseRef-LGPLLR",
        &["CHAT", "CHIEN"],
    )
}

pub fn without_third_party_notice(mut fixture: FixtureArtifact) -> FixtureArtifact {
    let workspace = TempDir::new().expect("notice fixture workspace");
    let decoder = flate2::read::GzDecoder::new(fixture.archive.as_slice());
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(workspace.path())
        .expect("unpack notice fixture");
    fs::remove_file(workspace.path().join("THIRD_PARTY_NOTICES")).expect("remove notice fixture");
    fixture.archive = archive_directory_without_notice(workspace.path());
    fixture.record.artifact_size_bytes =
        u64::try_from(fixture.archive.len()).expect("notice fixture size");
    fixture.record.artifact_sha256 = sha256_bytes(&fixture.archive);
    fixture
}

pub fn registry_with_urls(
    mut english: FixtureArtifact,
    mut french: FixtureArtifact,
    server: &TestServer,
) -> PackRegistry {
    english.record.artifact_url = server.url("/en.tar.gz");
    french.record.artifact_url = server.url("/fr.tar.gz");
    PackRegistry {
        schema_version: 1,
        packs: vec![english.record, french.record],
    }
}

pub fn write_registry(path: &Path, registry: &PackRegistry) {
    fs::write(
        path,
        toml::to_string_pretty(registry).expect("registry TOML"),
    )
    .expect("registry file");
}

fn describe_file(root: &Path, relative: &str) -> FileDescriptor {
    let bytes = fs::read(root.join(relative)).expect("fixture payload");
    FileDescriptor {
        path: relative.to_owned(),
        size_bytes: u64::try_from(bytes.len()).expect("fixture payload size"),
        sha256: sha256_bytes(&bytes),
    }
}

fn archive_directory(root: &Path) -> Vec<u8> {
    archive_files(
        root,
        &[
            "LICENSE",
            "SOURCE.md",
            "THIRD_PARTY_NOTICES",
            "curation/additions.toml",
            "curation/removals.toml",
            "lexicon.fst",
            "manifest.toml",
        ],
    )
}

fn archive_directory_without_notice(root: &Path) -> Vec<u8> {
    archive_files(
        root,
        &[
            "LICENSE",
            "SOURCE.md",
            "curation/additions.toml",
            "curation/removals.toml",
            "lexicon.fst",
            "manifest.toml",
        ],
    )
}

fn archive_files(root: &Path, relative_paths: &[&str]) -> Vec<u8> {
    let mut output = Vec::new();
    {
        let encoder = GzBuilder::new()
            .mtime(0)
            .operating_system(255)
            .write(&mut output, Compression::best());
        let mut archive = tar::Builder::new(encoder);
        for relative in relative_paths {
            let path = root.join(relative);
            let bytes = fs::read(&path).expect("archive payload");
            let mut header = tar::Header::new_gnu();
            header.set_path(relative).expect("portable fixture path");
            header.set_size(u64::try_from(bytes.len()).expect("archive entry size"));
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            archive
                .append(&header, bytes.as_slice())
                .expect("archive entry");
        }
        archive.finish().expect("fixture archive");
    }
    output
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[derive(Clone, Debug)]
pub enum ResponseBody {
    Complete(Vec<u8>),
    Interrupted {
        body: Vec<u8>,
        declared_length: usize,
    },
}

#[derive(Debug)]
pub struct TestServer {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    pub fn start(routes: BTreeMap<String, ResponseBody>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test server bind");
        listener
            .set_nonblocking(true)
            .expect("nonblocking test server");
        let address = listener.local_addr().expect("test server address");
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if thread_stop.load(Ordering::Acquire) {
                            break;
                        }
                        stream
                            .set_nonblocking(false)
                            .expect("blocking request stream");
                        serve_request(stream, &routes, &thread_requests);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("test server accept failed: {error}"),
                }
            }
        });
        Self {
            address,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Acquire)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("test server thread");
        }
    }
}

fn serve_request(
    mut stream: TcpStream,
    routes: &BTreeMap<String, ResponseBody>,
    requests: &AtomicUsize,
) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("request timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone request stream"));
    let mut first_line = String::new();
    reader.read_line(&mut first_line).expect("request line");
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).expect("request header") == 0 || line == "\r\n" {
            break;
        }
    }
    requests.fetch_add(1, Ordering::AcqRel);
    match routes.get(path) {
        Some(ResponseBody::Complete(body)) => {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("response headers");
            stream.write_all(body).expect("response body");
        }
        Some(ResponseBody::Interrupted {
            body,
            declared_length,
        }) => {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {declared_length}\r\nConnection: close\r\n\r\n"
            )
            .expect("response headers");
            stream.write_all(body).expect("partial response body");
        }
        None => {
            stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .expect("not found response");
        }
    }
}

#[derive(Debug)]
pub struct CliContext {
    _workspace: TempDir,
    pub root: PathBuf,
    pub data: PathBuf,
    pub registry: PathBuf,
    fake_bin: PathBuf,
}

impl CliContext {
    pub fn new() -> Self {
        let workspace = TempDir::new().expect("CLI fixture workspace");
        let root = workspace.path().to_path_buf();
        let fake_bin = root.join("bin");
        fs::create_dir(&fake_bin).expect("fake bin");
        let bun = fake_bin.join("bun");
        fs::write(
            &bun,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 1.3.0; fi\nexit 0\n",
        )
        .expect("fake Bun");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&bun, fs::Permissions::from_mode(0o755))
                .expect("fake Bun executable");
        }
        let data = root.join("data");
        let registry = root.join("registry.toml");
        Self {
            _workspace: workspace,
            root,
            data,
            registry,
            fake_bin,
        }
    }

    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_xtask"));
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.fake_bin.clone()];
        paths.extend(std::env::split_paths(&inherited_path));
        command
            .env(
                "PATH",
                std::env::join_paths(paths).expect("test command PATH"),
            )
            .env("WORD_ARENA_DATA_DIR", &self.data)
            .env("WORD_ARENA_PACK_REGISTRY", &self.registry)
            .env("WORD_ARENA_WORKSPACE_ROOT", &self.root);
        command
    }
}

pub fn read_file(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    File::open(path)
        .expect("open file")
        .read_to_end(&mut bytes)
        .expect("read file");
    bytes
}
