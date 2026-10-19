//! Assign callbacks to the compacted slots of successfully initialized workers.

/// Initialization failures consume an attempt, never a callback slot. Browser callbacks run
/// after this synchronous setup has returned, when every registered slot is in the vector.
pub(crate) fn initialized_workers<W>(attempts: usize, mut initialize: impl FnMut(usize) -> Option<W>, mut register: impl FnMut(usize, &W)) -> Vec<W> {
    let mut workers = Vec::new();
    for attempt in 0..attempts {
        if let Some(worker) = initialize(attempt) {
            register(workers.len(), &worker);
            workers.push(worker);
        }
    }
    workers
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct Worker {
        callback_slot: Cell<usize>,
        ready: bool,
        busy: Option<usize>,
        dead: bool,
    }

    #[test]
    fn survivors_route_ready_jobs_and_errors_after_initialization_failures() {
        // A constructor failure, an initialization-post failure, and failures between successes
        // all compact the vector. Deliver callbacks using the slots assigned in production.
        for failures in [vec![0], vec![0, 2], vec![1], vec![3]] {
            let mut workers =
                initialized_workers(4, |attempt| (!failures.contains(&attempt)).then(Worker::default), |slot, worker| worker.callback_slot.set(slot));
            let callbacks: Vec<_> = workers.iter().map(|w| w.callback_slot.get()).collect();
            for &slot in &callbacks {
                workers.get_mut(slot).unwrap().ready = true;
            }
            assert_eq!(workers.iter().filter(|w| w.ready).count(), 4 - failures.len());
            for (slot, worker) in workers.iter_mut().enumerate() {
                worker.busy = Some(slot + 10);
            }
            let mut completed = Vec::new();
            for &slot in callbacks.iter().rev() {
                completed.push(workers.get_mut(slot).unwrap().busy.take().unwrap());
            }
            assert_eq!(completed.len(), workers.len());
            assert!(workers.iter().all(|w| w.busy.is_none()));
            // The final survivor's error callback must reach that same slot too.
            let last = *callbacks.last().unwrap();
            workers.get_mut(last).unwrap().dead = true;
            assert!(workers.last().unwrap().dead);
            assert_eq!(workers.iter().filter(|w| w.dead).count(), 1);
        }
    }

    #[test]
    fn no_survivors_register_no_callbacks() {
        for attempts in [0, 4] {
            let workers: Vec<Worker> = initialized_workers(attempts, |_| None, |_, _| panic!("no worker to register"));
            assert!(workers.is_empty());
        }
    }
}
