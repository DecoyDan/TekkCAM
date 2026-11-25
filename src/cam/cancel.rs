//! Cooperative cancellation and progress reporting for long CAM jobs.
//!
//! The toolpath generators are plain functions that can run for minutes on a
//! big part. A job installs a [`Control`] for its thread; the generators call
//! [`check`] from their hot loops and the job can be stopped, and the UI shown
//! what it is doing, without changing any generator signature.

use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc,
};

pub const CANCELLED: &str = "cancelled";

#[derive(Clone)]
pub struct Control {
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<Sender<String>>,
}

thread_local! {
    static CONTROL: RefCell<Option<Control>> = const { RefCell::new(None) };
}

/// Keeps the control installed for the current thread until dropped.
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        CONTROL.with(|control| *control.borrow_mut() = None);
    }
}

pub fn install(control: Control) -> Guard {
    CONTROL.with(|slot| *slot.borrow_mut() = Some(control));
    Guard
}

pub fn is_cancelled() -> bool {
    CONTROL.with(|control| {
        control
            .borrow()
            .as_ref()
            .is_some_and(|control| control.cancel.load(Ordering::Relaxed))
    })
}

/// `Err(CANCELLED)` once the job has been cancelled.
pub fn check() -> Result<(), String> {
    if is_cancelled() {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

pub fn was_cancelled(error: &str) -> bool {
    error == CANCELLED
}

/// Tell the UI what the job is doing now.
pub fn progress(message: impl Into<String>) {
    CONTROL.with(|control| {
        if let Some(Control {
            progress: Some(sender),
            ..
        }) = control.borrow().as_ref()
        {
            let _ = sender.send(message.into());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_only_fails_after_cancel_on_the_installing_thread() {
        assert!(check().is_ok());
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::channel();
        let guard = install(Control {
            cancel: cancel.clone(),
            progress: Some(sender),
        });
        assert!(check().is_ok());
        progress("working");
        assert_eq!(receiver.try_recv().unwrap(), "working");
        cancel.store(true, Ordering::Relaxed);
        assert!(was_cancelled(&check().unwrap_err()));
        drop(guard);
        // Once the guard is gone the job no longer affects this thread.
        assert!(check().is_ok());
    }
}
