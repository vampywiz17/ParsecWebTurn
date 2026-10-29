//! Bounded FIFO with event-driven waiting; the predicate and notification share
//! one mutex so an event arriving just before sleep cannot be lost.
use std::{collections::VecDeque, sync::Mutex};
#[cfg(test)]
use std::{sync::Condvar, time::Instant};
const CAPACITY: usize = 256;
const MOTION_LIMIT: usize = CAPACITY - 64;

struct Policy<T> {
    merge: fn(&mut T, &T) -> bool,
    motion: fn(&T) -> bool,
    overflow: fn() -> T,
}

struct State<T> {
    events: VecDeque<T>,
    stopped: bool,

    consumer: Option<std::thread::Thread>,
}

pub struct Queue<T> {
    state: Mutex<State<T>>,
    policy: Option<Policy<T>>,
    #[cfg(test)]
    changed: Condvar,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            policy: None,
            state: Mutex::new(State {
                events: VecDeque::with_capacity(CAPACITY),
                stopped: false,

                consumer: None,
            }),
            #[cfg(test)]
            changed: Condvar::new(),
        }
    }
}

impl<T> Queue<T> {
    pub fn push(&self, event: T) {
        self.try_push(event);
    }

    pub fn try_push(&self, event: T) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped {
            return false;
        }
        if let Some(policy) = &self.policy {
            if state
                .events
                .back_mut()
                .is_some_and(|tail| (policy.merge)(tail, &event))
            {
                return true;
            }
            let limit = if (policy.motion)(&event) {
                MOTION_LIMIT
            } else {
                CAPACITY
            };
            if state.events.len() >= limit {
                // No silent loss of input or releases. Invalidate stale work and
                // ask the existing session consumer to release and disconnect.
                state.events.clear();
                state.events.push_back((policy.overflow)());
                state.stopped = true;
                if let Some(consumer) = &state.consumer {
                    consumer.unpark();
                }
                drop(state);
                #[cfg(test)]
                self.changed.notify_one();
                return false;
            }
        }
        if !state.stopped && state.events.len() < CAPACITY {
            state.events.push_back(event);

            if let Some(consumer) = &state.consumer {
                consumer.unpark();
            }
            drop(state);
            #[cfg(test)]
            self.changed.notify_one();
            true
        } else {
            false
        }
    }

    pub fn set_consumer(&self, consumer: std::thread::Thread) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .consumer = Some(consumer);
    }

    pub fn wake_consumer(&self) {
        if let Some(consumer) = &self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .consumer
        {
            consumer.unpark();
        }
    }

    /// Swap storage in constant time; the consumer processes outside the input
    /// mutex. Both bounded allocations are retained across successive batches.
    pub fn take_batch(&self, batch: &mut VecDeque<T>) {
        assert!(
            batch.is_empty(),
            "input batch must be consumed before reuse"
        );
        batch.reserve(CAPACITY);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::swap(&mut state.events, batch);
    }

    #[cfg(test)]
    pub fn drain(&self) -> Vec<T> {
        let mut batch = VecDeque::new();
        self.take_batch(&mut batch);
        batch.into_iter().collect()
    }

    #[cfg(test)]
    pub fn wait_until(&self, deadline: Instant) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        drop(
            self.changed
                .wait_timeout_while(
                    state,
                    deadline.saturating_duration_since(Instant::now()),
                    |s| s.events.is_empty() && !s.stopped,
                )
                .unwrap_or_else(|e| e.into_inner()),
        );
    }

    pub fn stop(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;

        self.wake_consumer();
        #[cfg(test)]
        self.changed.notify_all();
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl Queue<crate::native_core::session_model::Event> {
    pub fn input() -> Self {
        use crate::native_core::session_model::Event;
        Self {
            policy: Some(Policy {
                merge: Event::merge_motion,
                motion: Event::is_motion,
                overflow: || Event::InputOverflow,
            }),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{mpsc, Arc},
        time::Duration,
    };

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn pointer_flood_keeps_latest_positions_and_all_drag_boundaries() {
        use crate::native_core::session_model::Event;
        let queue = Queue::input();
        queue.push(Event::Key(true, "ShiftLeft", 1));
        for x in 0..100_000 {
            queue.push(Event::Motion(x, 10));
        }
        queue.push(Event::Button(true, 0, 99_999, 10));
        for y in 0..100_000 {
            queue.push(Event::Motion(99_999, y));
        }
        queue.push(Event::Button(false, 0, 99_999, 99_999));
        queue.push(Event::Key(false, "ShiftLeft", 0));
        let events = queue.drain();
        assert_eq!(events.len(), 6, "obsolete positions accumulated");
        assert!(matches!(events[0], Event::Key(true, "ShiftLeft", 1)));
        assert!(matches!(events[1], Event::Motion(99_999, 10)));
        assert!(matches!(events[2], Event::Button(true, 0, 99_999, 10)));
        assert!(matches!(events[3], Event::Motion(99_999, 99_999)));
        assert!(matches!(events[4], Event::Button(false, 0, 99_999, 99_999)));
        assert!(matches!(events[5], Event::Key(false, "ShiftLeft", 0)));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn camera_deltas_are_exact_and_never_cross_mode_focus_or_wheel_boundaries() {
        use crate::native_core::session_model::Event;
        let queue = Queue::input();
        for _ in 0..100_000 {
            queue.push(Event::RelativeMotion(3, -2));
        }
        queue.push(Event::Scroll(0, 120));
        queue.push(Event::RelativeMotion(-4, 5));
        queue.push(Event::RelativeMode(false));
        queue.push(Event::Motion(10, 20));
        queue.push(Event::Size(1920, 1080));
        queue.push(Event::Motion(30, 40));
        queue.push(Event::Focus(false));
        queue.push(Event::RelativeMotion(1, 1));
        let events = queue.drain();
        assert_eq!(events.len(), 9);
        assert!(matches!(
            events[0],
            Event::RelativeMotion(300_000, -200_000)
        ));
        assert!(matches!(events[1], Event::Scroll(0, 120)));
        assert!(matches!(events[2], Event::RelativeMotion(-4, 5)));
        assert!(matches!(events[3], Event::RelativeMode(false)));
        assert!(matches!(events[4], Event::Motion(10, 20)));
        assert!(matches!(events[5], Event::Size(1920, 1080)));
        assert!(matches!(events[6], Event::Motion(30, 40)));
        assert!(matches!(events[7], Event::Focus(false)));
        assert!(matches!(events[8], Event::RelativeMotion(1, 1)));

        // Overflow in either coordinate must not saturate or partly mutate it.
        queue.push(Event::RelativeMotion(i32::MAX, 7));
        queue.push(Event::RelativeMotion(1, i32::MIN));
        let events = queue.drain();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::RelativeMotion(i32::MAX, 7)));
        assert!(matches!(events[1], Event::RelativeMotion(1, i32::MIN)));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn reserved_input_capacity_and_overflow_release_barrier_are_bounded() {
        use crate::native_core::session_model::Event;
        let queue = Queue::input();
        // Deliberately unmergeable modes model pathological producer pressure.
        for i in 0..MOTION_LIMIT {
            assert!(queue.try_push(if i % 2 == 0 {
                Event::Motion(1, 2)
            } else {
                Event::RelativeMotion(1, 2)
            }));
        }
        for _ in MOTION_LIMIT..CAPACITY {
            assert!(queue.try_push(Event::Key(false, "KeyA", 0)));
        }
        assert_eq!(queue.drain().len(), CAPACITY);
        for _ in 0..CAPACITY {
            assert!(queue.try_push(Event::Key(true, "KeyA", 0)));
        }
        assert!(!queue.try_push(Event::Key(false, "KeyA", 0)));
        assert!(!queue.try_push(Event::Motion(9, 9)));
        assert!(matches!(queue.drain().as_slice(), [Event::InputOverflow]));
        assert!(queue.drain().is_empty());
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn flood_while_consumer_is_blocked_keeps_owned_batch_and_next_sum() {
        use crate::native_core::session_model::Event;
        let queue = Arc::new(Queue::input());
        queue.push(Event::Button(true, 0, 0, 0));
        let q = queue.clone();
        let (ready, started) = mpsc::channel();
        let (resume, proceed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut batch = VecDeque::new();
            q.set_consumer(std::thread::current());
            q.take_batch(&mut batch);
            ready.send(()).unwrap();
            proceed.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(matches!(
                batch.pop_front(),
                Some(Event::Button(true, 0, 0, 0))
            ));
            q.take_batch(&mut batch);
            assert!(matches!(
                batch.pop_front(),
                Some(Event::RelativeMotion(100_000, -100_000))
            ));
            assert!(matches!(
                batch.pop_front(),
                Some(Event::Button(false, 0, 0, 0))
            ));
            assert!(batch.is_empty());
        });
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..100_000 {
            queue.push(Event::RelativeMotion(1, -1));
        }
        queue.push(Event::Button(false, 0, 0, 0));
        resume.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn native_consumer_is_woken_for_input_and_stop_without_polling() {
        let queue = Arc::new(Queue::<u8>::default());
        let (ready, started) = mpsc::channel();
        let q = queue.clone();
        let worker = std::thread::spawn(move || {
            q.set_consumer(std::thread::current());
            ready.send(()).unwrap();
            let started = Instant::now();
            std::thread::park_timeout(Duration::from_secs(1));
            assert!(started.elapsed() < Duration::from_millis(500));
            q.drain()
        });
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        queue.push(7);
        assert_eq!(worker.join().unwrap(), vec![7]);
        queue.stop();
        assert!(!queue.try_push(9));
    }

    #[test]
    fn repeated_full_batches_reuse_two_allocations_and_preserve_fifo() {
        let queue = Queue::default();
        let mut batch = VecDeque::new();
        let mut addresses = std::collections::BTreeSet::new();
        for round in 0..128 {
            for item in 0..CAPACITY {
                queue.push(round * CAPACITY + item);
            }
            queue.take_batch(&mut batch);
            assert_eq!(batch.len(), CAPACITY);
            addresses.insert(batch.as_slices().0.as_ptr() as usize);
            for item in 0..CAPACITY {
                assert_eq!(batch.pop_front(), Some(round * CAPACITY + item));
            }
        }
        assert_eq!(addresses.len(), 2, "input storage was reallocated");
    }

    #[test]
    fn producer_can_fill_next_batch_while_consumer_owns_previous_one() {
        let queue = Arc::new(Queue::default());
        queue.push(1);
        let mut batch = VecDeque::new();
        queue.take_batch(&mut batch);
        let producer = queue.clone();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            producer.push(2);
            producer.push(3);
            tx.send(()).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(batch.pop_front(), Some(1));
        queue.take_batch(&mut batch);
        assert_eq!(batch.into_iter().collect::<Vec<_>>(), vec![2, 3]);
        worker.join().unwrap();
    }

    #[test]
    fn queued_events_and_shutdown_are_not_lost_before_wait() {
        for stopped in [false, true] {
            let queue = Arc::new(Queue::default());
            if stopped {
                queue.stop();
            } else {
                queue.push(7);
            }
            let worker = queue.clone();
            let (tx, rx) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                worker.wait_until(Instant::now() + Duration::from_secs(5));
                tx.send(worker.drain()).unwrap();
            });
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                if stopped { vec![] } else { vec![7] }
            );
            thread.join().unwrap();
        }
    }

    #[test]
    fn arrival_and_shutdown_interrupt_wait_but_empty_notifications_do_not() {
        for stopped in [false, true] {
            let queue = Arc::new(Queue::<u32>::default());
            let worker = queue.clone();
            let (tx, rx) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                worker.wait_until(Instant::now() + Duration::from_secs(5));
                tx.send(worker.drain()).unwrap();
            });
            queue.changed.notify_all();
            assert!(matches!(
                rx.recv_timeout(Duration::from_millis(20)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            if stopped {
                queue.stop();
            } else {
                queue.push(9);
            }
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                if stopped { vec![] } else { vec![9] }
            );
            thread.join().unwrap();
        }
    }

    #[test]
    fn capacity_order_and_idle_deadline_are_preserved() {
        let queue = Queue::default();
        for i in 0..300 {
            assert_eq!(queue.try_push(i), i < 256);
        }
        assert_eq!(queue.drain(), (0..256).collect::<Vec<_>>());
        let deadline = Instant::now() + Duration::from_millis(10);
        queue.wait_until(deadline);
        assert!(Instant::now() >= deadline);
        queue.stop();
        assert!(!queue.try_push(42));
        assert!(queue.drain().is_empty());
    }
}
