use {
    super::super::task::{waker_ref, ArcWake},
    std::{
        future::Future,
        sync::{atomic::AtomicBool, Arc},
        task::{Context, Poll},
        thread,
        thread::Thread,
    },
};

/// Runs the given future to completion on the current thread.
/// 
/// This function will block until the given future has completed.
pub fn block_on<F: Future>(mut f: F) -> <F as Future>::Output {
    use std::pin::Pin;

    let mut f = unsafe { Pin::new_unchecked(&mut f) };
    run_executor(|cx| f.as_mut().poll(cx))
}

fn run_executor<T, F: FnMut(&mut Context<'_>) -> Poll<T>>(mut f: F) -> T {
    use std::sync::atomic::Ordering;

    thread_local! {
        static CURRENT_THREAD_NOTIFIER: Arc<ThreadNotifier> = Arc::new(ThreadNotifier {
            thread: thread::current(),
            woken: AtomicBool::new(false),
        });
    }

    #[derive(Debug)]
    struct ThreadNotifier {
        thread: Thread,
        woken: AtomicBool,
    }

    impl ArcWake for ThreadNotifier {
        fn wake_by_ref(self: &Arc<Self>) {
            let woken = self.woken.swap(true, Ordering::Release);
            if !woken {
                self.thread.unpark();
            }
        }
    }
    let _enter = super::enter().unwrap();
   
    CURRENT_THREAD_NOTIFIER.with(|notifier| {
        let waker = waker_ref(notifier);
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(t) = f(&mut cx) {
                return t;
            }
            while !notifier.woken.swap(false, Ordering::Acquire) {
                thread::park();
            }
        }
    })
}
