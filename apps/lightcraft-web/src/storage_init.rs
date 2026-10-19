//! Select storage once: operational failures never select another library's backend.

use std::future::Future;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Opfs,
    Idb,
}

impl Kind {
    pub(crate) fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "opfs" => Ok(Self::Opfs),
            "idb" => Ok(Self::Idb),
            _ => Err(format!("unsupported persistent storage backend: {name}")),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Request {
    Automatic,
    Required(Kind),
}

/// Only missing capability permits automatic IndexedDB selection. The opener runs once;
/// its error belongs to the selected library and must reach startup or worker admission.
pub(crate) async fn open<T, F: Future<Output = Result<T, String>>>(
    request: Request,
    opfs_available: bool,
    opener: impl FnOnce(Kind) -> F,
) -> Result<T, String> {
    let kind = match request {
        Request::Automatic => {
            if opfs_available {
                Kind::Opfs
            } else {
                Kind::Idb
            }
        }
        Request::Required(Kind::Opfs) if !opfs_available => return Err("the required OPFS storage is not supported here".into()),
        Request::Required(kind) => kind,
    };
    opener(kind).await
}

/// A ready message certifies the backend that actually opened, including temporary workers.
pub(crate) fn check_ready(expected: &str, actual: &str) -> Result<(), String> {
    if !matches!(expected, "opfs" | "idb" | "memory") || expected != actual {
        return Err(format!("render worker storage mismatch: expected {expected}, opened {actual}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;
    use std::task::{Context, Poll, Waker};

    // Fixture openers are immediately ready; no executor, browser, engine or thread is needed.
    fn resolved<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("fixture opener must be immediately ready"),
        }
    }

    #[test]
    fn root_refusal_never_opens_an_alternative_and_retry_reads_the_same_library() {
        let healthy_catalog = b"healthy existing OPFS catalog".to_vec();
        let mut attempts = Vec::new();
        let refused: Result<Vec<u8>, String> = resolved(open(Request::Automatic, true, |kind| {
            attempts.push(kind);
            ready(Err("fixture OPFS root refusal".into()))
        }));
        assert_eq!(refused.unwrap_err(), "fixture OPFS root refusal");
        assert_eq!(attempts, [Kind::Opfs]);
        let reopened = resolved(open(Request::Automatic, true, |kind| {
            attempts.push(kind);
            ready(Ok(healthy_catalog.clone()))
        }))
        .unwrap();
        assert_eq!(reopened, healthy_catalog);
        assert_eq!(attempts, [Kind::Opfs, Kind::Opfs]);
    }

    #[test]
    fn absent_opfs_capability_selects_indexeddb_once() {
        let mut attempts = Vec::new();
        let result = resolved(open(Request::Automatic, false, |kind| {
            attempts.push(kind);
            ready(Ok("healthy IndexedDB library"))
        }))
        .unwrap();
        assert_eq!(result, "healthy IndexedDB library");
        assert_eq!(attempts, [Kind::Idb]);
    }

    #[test]
    fn explicit_indexeddb_does_not_try_opfs() {
        let mut attempts = Vec::new();
        resolved(open(Request::Required(Kind::Idb), true, |kind| {
            attempts.push(kind);
            ready(Ok(()))
        }))
        .unwrap();
        assert_eq!(attempts, [Kind::Idb]);
    }

    #[test]
    fn opfs_worker_without_capability_never_opens_indexeddb() {
        let mut attempts = Vec::new();
        let result = resolved(open(Request::Required(Kind::Opfs), false, |kind| {
            attempts.push(kind);
            ready(Ok(()))
        }));
        assert!(result.unwrap_err().contains("required OPFS"));
        assert!(attempts.is_empty());
    }

    #[test]
    fn required_worker_storage_failure_stays_an_error() {
        for kind in [Kind::Opfs, Kind::Idb] {
            let mut attempts = Vec::new();
            let result: Result<(), String> = resolved(open(Request::Required(kind), true, |actual| {
                attempts.push(actual);
                ready(Err("fixture worker storage refusal".into()))
            }));
            assert_eq!(result.unwrap_err(), "fixture worker storage refusal");
            assert_eq!(attempts, [kind]);
        }
    }

    #[test]
    fn missing_or_different_worker_storage_cannot_be_ready() {
        for expected in ["opfs", "idb"] {
            for actual in ["", "memory", if expected == "opfs" { "idb" } else { "opfs" }] {
                assert!(check_ready(expected, actual).is_err(), "{expected}/{actual}");
            }
        }
        assert!(Kind::from_name("memory").is_err());
        assert!(check_ready("unknown", "unknown").is_err());
    }

    #[test]
    fn matching_worker_storage_is_ready() {
        for kind in ["opfs", "idb", "memory"] {
            assert_eq!(check_ready(kind, kind), Ok(()));
        }
        assert_eq!(Kind::from_name("opfs"), Ok(Kind::Opfs));
        assert_eq!(Kind::from_name("idb"), Ok(Kind::Idb));
    }
}
