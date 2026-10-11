//! Backing-track waveform cache (M21-A).
//!
//! The editor draws the backing track's loudness and onset curves behind the
//! grid. Decoding + analysing a full song takes about a second, so it runs on a
//! background thread the first time the waveform of a backing path is asked
//! for; until it lands the query answers `pending`. The cache holds the one
//! analysis for the currently attached backing path: a query for another path
//! (attach/replace/load) starts over, and a finished analysis for a path that
//! is no longer the current one is discarded.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rockcraft_core::{analyze_waveform, Waveform, WAVEFORM_BUCKET_US};
use serde::Serialize;

/// The state of one path's analysis.
#[derive(Debug, Clone)]
enum Slot {
    Pending,
    Ready(Arc<Waveform>),
    Failed(String),
}

/// Reply of the `backing_waveform` host command / `edit_backing_waveform` IPC.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WaveformReply {
    /// No backing track attached.
    None,
    /// Analysis still running; ask again shortly.
    Pending,
    /// The backing could not be decoded.
    Failed { detail: String },
    /// The curves, indexed by backing-file position (see [`Waveform`]).
    Ready {
        file: String,
        bucket_us: u64,
        envelope: Vec<u8>,
        onsets: Vec<u8>,
    },
}

/// Analysis of the current backing path, shared with its worker thread.
#[derive(Debug, Default, Clone)]
pub struct WaveformCache {
    inner: Arc<Mutex<Option<(PathBuf, Slot)>>>,
}

impl WaveformCache {
    /// The waveform for `backing` (`None` = no backing attached), starting the
    /// analysis on a background thread if this path hasn't been analysed yet.
    pub fn query(&self, backing: Option<&Path>) -> WaveformReply {
        self.query_with(backing, decode_and_analyze)
    }

    /// [`query`](Self::query) with an injectable analysis, for tests.
    fn query_with<F>(&self, backing: Option<&Path>, compute: F) -> WaveformReply
    where
        F: FnOnce(&Path) -> Result<Waveform, String> + Send + 'static,
    {
        let mut guard = self.inner.lock().expect("waveform cache poisoned");
        let Some(path) = backing else {
            *guard = None;
            return WaveformReply::None;
        };
        match guard.as_ref() {
            Some((p, slot)) if p == path => return reply_of(path, slot),
            _ => {}
        }
        *guard = Some((path.to_path_buf(), Slot::Pending));
        drop(guard);

        let inner = Arc::clone(&self.inner);
        let owned = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("rockcraft-waveform".into())
            .spawn(move || {
                let slot = match compute(&owned) {
                    Ok(w) => Slot::Ready(Arc::new(w)),
                    Err(e) => Slot::Failed(e),
                };
                let mut guard = inner.lock().expect("waveform cache poisoned");
                // Discard the result if the backing changed meanwhile.
                if let Some((p, s)) = guard.as_mut() {
                    if *p == owned {
                        *s = slot;
                    }
                }
            });
        if let Err(e) = spawned {
            let detail = format!("could not start the waveform thread: {e}");
            *self.inner.lock().expect("waveform cache poisoned") =
                Some((path.to_path_buf(), Slot::Failed(detail.clone())));
            return WaveformReply::Failed { detail };
        }
        WaveformReply::Pending
    }
}

fn reply_of(path: &Path, slot: &Slot) -> WaveformReply {
    match slot {
        Slot::Pending => WaveformReply::Pending,
        Slot::Failed(detail) => WaveformReply::Failed {
            detail: detail.clone(),
        },
        Slot::Ready(w) => WaveformReply::Ready {
            file: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bucket_us: w.bucket_us,
            envelope: w.envelope.clone(),
            onsets: w.onsets.clone(),
        },
    }
}

fn decode_and_analyze(path: &Path) -> Result<Waveform, String> {
    let track = rockcraft_audio::DecodedTrack::load(path).map_err(|e| e.to_string())?;
    Ok(analyze_waveform(
        track.samples(),
        track.channels(),
        track.sample_rate(),
        WAVEFORM_BUCKET_US,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn fake(envelope: Vec<u8>) -> impl FnOnce(&Path) -> Result<Waveform, String> + Send {
        move |_| {
            Ok(Waveform {
                bucket_us: WAVEFORM_BUCKET_US,
                onsets: vec![0; envelope.len()],
                envelope,
            })
        }
    }

    /// Poll until the reply stops being `pending` (the worker is a thread).
    fn settle(cache: &WaveformCache, path: &Path) -> WaveformReply {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let r = cache.query_with(Some(path), |_| Err("must not recompute".into()));
            if r != WaveformReply::Pending || Instant::now() > deadline {
                return r;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn none_then_pending_then_ready_then_none_after_detach() {
        let cache = WaveformCache::default();
        assert_eq!(cache.query_with(None, fake(vec![])), WaveformReply::None);
        let path = PathBuf::from("/bundle/backing.wav");
        assert_eq!(
            cache.query_with(Some(&path), fake(vec![1, 2, 3])),
            WaveformReply::Pending
        );
        match settle(&cache, &path) {
            WaveformReply::Ready { file, envelope, .. } => {
                assert_eq!(file, "backing.wav");
                assert_eq!(envelope, vec![1, 2, 3]);
            }
            other => panic!("expected ready, got {other:?}"),
        }
        assert_eq!(cache.query_with(None, fake(vec![])), WaveformReply::None);
    }

    #[test]
    fn a_new_path_recomputes_and_a_decode_error_is_reported() {
        let cache = WaveformCache::default();
        let a = PathBuf::from("/a.wav");
        cache.query_with(Some(&a), fake(vec![9]));
        assert!(matches!(settle(&cache, &a), WaveformReply::Ready { .. }));
        let b = PathBuf::from("/b.wav");
        assert_eq!(
            cache.query_with(Some(&b), |_| Err("bad file".into())),
            WaveformReply::Pending
        );
        assert_eq!(
            settle(&cache, &b),
            WaveformReply::Failed {
                detail: "bad file".into()
            }
        );
    }

    #[test]
    fn a_stale_result_for_a_replaced_path_is_discarded() {
        let cache = WaveformCache::default();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let old = PathBuf::from("/old.wav");
        cache.query_with(Some(&old), move |_| {
            let _ = rx.recv();
            Ok(Waveform {
                bucket_us: WAVEFORM_BUCKET_US,
                envelope: vec![7],
                onsets: vec![7],
            })
        });
        let new = PathBuf::from("/new.wav");
        cache.query_with(Some(&new), fake(vec![1]));
        tx.send(()).unwrap();
        match settle(&cache, &new) {
            WaveformReply::Ready { envelope, .. } => assert_eq!(envelope, vec![1]),
            other => panic!("expected ready, got {other:?}"),
        }
    }
}
