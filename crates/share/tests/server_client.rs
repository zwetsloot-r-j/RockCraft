//! End-to-end: a real share server on an ephemeral port, driven by the client
//! and by `ShareService`, against temp libraries (M26-B).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rockcraft_core::{MidiNote, Note, Timeline, Velocity};
use rockcraft_share::{client, ShareError, ShareServer, ShareService, TransferState};

/// A bundle `name` under `root` with `notes` notes, a wav backing and a video.
fn make_bundle(root: &Path, name: &str, notes: u8) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    let mut tl = Timeline::new();
    for i in 0..notes {
        tl.insert(Note {
            pitch: MidiNote::new(60 + i).unwrap(),
            start_us: i as u64 * 500_000,
            dur_us: 400_000,
            velocity: Velocity::new(80).unwrap(),
            hand: None,
        });
    }
    fs::write(
        dir.join("song.mid"),
        rockcraft_midi::events_to_smf_bytes(&tl.to_events()),
    )
    .unwrap();
    fs::write(
        dir.join("meta.json"),
        r#"{"midi_file":"song.mid",
            "backing":{"file":"backing.wav","audio_start_us":0},
            "video":{"file":"source.mp4","offset_us":0}}"#,
    )
    .unwrap();
    fs::write(dir.join("backing.wav"), vec![1u8; 300_000]).unwrap();
    fs::write(dir.join("source.mp4"), vec![2u8; 200_000]).unwrap();
    dir
}

/// A server sharing `<tmp>/a/library` with two songs. Returns (server, addr, tmp).
fn serve() -> (ShareServer, String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("a").join("library");
    make_bundle(&library, "frog-song", 3);
    make_bundle(&library, "waltz", 5);
    let server = ShareServer::start(Some(0), vec![library.clone()], library, "peer-a".into())
        .expect("server starts");
    let addr = format!("127.0.0.1:{}", server.port());
    (server, addr, tmp)
}

fn same_files(a: &Path, b: &Path) {
    for f in ["meta.json", "song.mid", "backing.wav", "source.mp4"] {
        assert_eq!(
            fs::read(a.join(f)).unwrap(),
            fs::read(b.join(f)).unwrap(),
            "{f}"
        );
    }
}

#[test]
fn info_and_list() {
    let (_server, addr, _tmp) = serve();
    let info = client::info(&addr).unwrap();
    assert_eq!(
        (info.app.as_str(), info.api, info.songs),
        ("rockcraft", 1, 2)
    );
    assert_eq!(info.name, "peer-a");

    let songs = client::list(&addr).unwrap();
    let names: Vec<&str> = songs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["frog-song", "waltz"]);
    let frog = &songs[0];
    assert_eq!(frog.id, "library~frog-song");
    assert_eq!(frog.note_count, 3);
    assert!(frog.has_backing && frog.has_video);
    assert!(frog.size_bytes > 500_000);
    assert_eq!(frog.chart_hash.len(), 64);
    assert_ne!(frog.chart_hash, songs[1].chart_hash);
}

#[test]
fn download_with_progress_and_conflicts() {
    let (_server, addr, tmp) = serve();
    let source = tmp.path().join("a").join("library").join("frog-song");
    let library = tmp.path().join("b").join("library");
    let song = client::list(&addr).unwrap().remove(0);

    let last = std::sync::Mutex::new((0u64, 0u64));
    let dir = client::download(&addr, &song, &library, false, None, &|d, t| {
        *last.lock().unwrap() = (d, t);
    })
    .unwrap();
    assert_eq!(dir, library.join("frog-song"));
    same_files(&source, &dir);
    let (done, total) = *last.lock().unwrap();
    assert!(
        done > 0 && done == total,
        "progress ends at total: {done}/{total}"
    );

    let err = client::download(&addr, &song, &library, false, None, &|_, _| {}).unwrap_err();
    assert_eq!(err.to_string(), "exists:frog-song");
    client::download(&addr, &song, &library, true, None, &|_, _| {}).unwrap();
    let kept = client::download(
        &addr,
        &song,
        &library,
        false,
        Some("frog-song-2"),
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(kept, library.join("frog-song-2"));

    let mut ghost = song.clone();
    ghost.id = "library~nope".into();
    ghost.name = "nope".into();
    let err = client::download(&addr, &ghost, &library, false, None, &|_, _| {}).unwrap_err();
    assert!(matches!(err, ShareError::NotFound(_)), "{err}");
}

#[test]
fn upload_conflicts_and_bad_requests() {
    let (_server, addr, tmp) = serve();
    let mine = make_bundle(&tmp.path().join("b"), "new-tune", 4);
    let served = tmp.path().join("a").join("library");

    assert_eq!(
        client::upload(&addr, &mine, false, None, &|_, _| {}).unwrap(),
        "new-tune"
    );
    same_files(&mine, &served.join("new-tune"));
    let err = client::upload(&addr, &mine, false, None, &|_, _| {}).unwrap_err();
    assert_eq!(err.to_string(), "exists:new-tune");
    client::upload(&addr, &mine, true, None, &|_, _| {}).unwrap();
    assert_eq!(client::list(&addr).unwrap().len(), 3);

    // A name that isn't its own slug, and a body that isn't an archive.
    let put =
        |slug: &str, body: &[u8]| match ureq::put(&format!("http://{addr}/api/v1/songs/{slug}"))
            .send_bytes(body)
        {
            Ok(r) => r.status(),
            Err(ureq::Error::Status(code, _)) => code,
            Err(e) => panic!("{e}"),
        };
    assert_eq!(put("Bad%20Name", b"x"), 400);
    assert_eq!(put("garbage", b"not a zip at all"), 400);
    assert!(!served.join("garbage").exists());
    assert!(!served.join("garbage.incoming").exists());
}

/// Poll `svc` until transfer `id` leaves `Running` (10 s cap).
fn wait(svc: &ShareService, id: u64) -> TransferState {
    let start = Instant::now();
    loop {
        let t = svc
            .transfers()
            .into_iter()
            .find(|t| t.id == id)
            .expect("row");
        if t.state != TransferState::Running {
            return t.state;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "transfer stuck");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn service_lists_downloads_and_uploads_in_the_background() {
    let (_server, addr, tmp) = serve();
    let library = tmp.path().join("b").join("library");
    make_bundle(&library, "waltz", 2); // same name, other chart
    let svc = ShareService::new(vec![library.clone()], library.clone());

    let listed = svc.list(&addr).unwrap();
    let by = |n: &str| listed.iter().find(|s| s.song.name == n).unwrap().local;
    assert_eq!(by("frog-song"), rockcraft_share::LocalState::Absent);
    assert_eq!(by("waltz"), rockcraft_share::LocalState::Different);

    let id = svc.download(&addr, "library~frog-song", false, None);
    assert!(matches!(
        wait(&svc, id),
        TransferState::Done { dir: Some(_) }
    ));
    let row = svc.transfers().into_iter().find(|t| t.id == id).unwrap();
    assert!(row.bytes_done > 0 && row.bytes_done == row.bytes_total);
    let listed = svc.list(&addr).unwrap();
    assert_eq!(
        listed
            .iter()
            .find(|s| s.song.name == "frog-song")
            .unwrap()
            .local,
        rockcraft_share::LocalState::Same
    );

    let id = svc.download(&addr, "library~waltz", false, None);
    assert_eq!(
        wait(&svc, id),
        TransferState::Exists {
            slug: "waltz".into()
        }
    );

    let id = svc.upload(&addr, library.join("waltz").to_str().unwrap(), false, None);
    assert_eq!(
        wait(&svc, id),
        TransferState::Exists {
            slug: "waltz".into()
        }
    );
    let id = svc.upload(
        &addr,
        library.join("waltz").to_str().unwrap(),
        false,
        Some("waltz-2".into()),
    );
    assert!(matches!(wait(&svc, id), TransferState::Done { dir: None }));
}

#[test]
fn unreachable_server_is_a_clean_error() {
    let err = client::list("127.0.0.1:1").unwrap_err();
    assert!(matches!(err, ShareError::Unreachable { .. }), "{err}");
}
