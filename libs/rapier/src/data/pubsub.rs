//! Publish-subscribe mechanism for internal events.

use std::collections::VecDeque;
use std::marker::PhantomData;

/// A permanent subscription to a pub-sub queue.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone)]
pub struct Subscription<T> {
    // Position on the cursor array.
    id: u32,
    _phantom: PhantomData<T>,
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone)]
struct PubSubCursor {
    // Position on the offset array.
    id: u32,
    // Index of the next message to read.
    // NOTE: Having this here is not actually necessary because
    // this value is supposed to be equal to `offsets[self.id]`.
    // However, we keep it because it lets us avoid one lookup
    // on the `offsets` array inside of message-polling loops
    // based on `read_ith`.
    next: u32,
}

impl PubSubCursor {
    fn id(&self, num_deleted: u32) -> usize {
        (self.id - num_deleted) as usize
    }

    fn next(&self, num_deleted: u32) -> usize {
        (self.next - num_deleted) as usize
    }
}

/// A pub-sub queue.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Default)]
pub struct PubSub<T> {
    deleted_messages: u32,
    deleted_offsets: u32,
    messages: VecDeque<T>,
    offsets: VecDeque<u32>,
    cursors: Vec<PubSubCursor>,
}

impl<T> PubSub<T> {
    /// Create a new empty pub-sub queue.
    pub fn new() -> Self {
        Self {
            deleted_offsets: 0,
            deleted_messages: 0,
            messages: VecDeque::new(),
            offsets: VecDeque::new(),
            cursors: Vec::new(),
        }
    }

    fn reset_shifts(&mut self) {
        for offset in &mut self.offsets {
            *offset -= self.deleted_messages;
        }

        for cursor in &mut self.cursors {
            cursor.id -= self.deleted_offsets;
            cursor.next -= self.deleted_messages;
        }

        self.deleted_offsets = 0;
        self.deleted_messages = 0;
    }

    /// Publish a new message.
    pub fn publish(&mut self, message: T) {
        if self.offsets.is_empty() {
            // No subscribers, drop the message.
            return;
        }

        self.messages.push_back(message);
    }

    /// Subscribe to the queue.
    ///
    /// A subscription cannot be cancelled.
    #[must_use]
    pub fn subscribe(&mut self) -> Subscription<T> {
        let cursor = PubSubCursor {
            next: self.messages.len() as u32 + self.deleted_messages,
            id: self.offsets.len() as u32 + self.deleted_offsets,
        };

        let subscription = Subscription {
            id: self.cursors.len() as u32,
            _phantom: PhantomData,
        };

        self.offsets.push_back(cursor.next);
        self.cursors.push(cursor);
        subscription
    }

    /// Read the i-th message not yet read by the given subscriber.
    pub fn read_ith(&self, sub: &Subscription<T>, i: usize) -> Option<&T> {
        let cursor = &self.cursors[sub.id as usize];
        self.messages.get(cursor.next(self.deleted_messages) + i)
    }

    /// Get the messages not yet read by the given subscriber.
    pub fn read(&self, sub: &Subscription<T>) -> impl Iterator<Item = &T> {
        let cursor = &self.cursors[sub.id as usize];
        let next = cursor.next(self.deleted_messages);

        self.messages.range(next..)
    }

    /// Makes the given subscribe acknowledge all the messages in the queue.
    ///
    /// A subscriber cannot read acknowledged messages any more.
    pub fn ack(&mut self, sub: &Subscription<T>) {
        // Update the cursor.
        let cursor = &mut self.cursors[sub.id as usize];

        self.offsets[cursor.id(self.deleted_offsets)] = u32::MAX;
        cursor.id = self.offsets.len() as u32 + self.deleted_offsets;

        cursor.next = self.messages.len() as u32 + self.deleted_messages;
        self.offsets.push_back(cursor.next);

        // Now clear the messages we don't need to
        // maintain in memory anymore.
        while self.offsets.front() == Some(&u32::MAX) {
            self.offsets.pop_front();
            self.deleted_offsets += 1;
        }

        // There must be at least one offset otherwise
        // that would mean we have no subscribers.
        let next = self.offsets.front().unwrap();
        let num_to_delete = *next - self.deleted_messages;

        for _ in 0..num_to_delete {
            self.messages.pop_front();
        }

        self.deleted_messages += num_to_delete;

        if self.deleted_messages > u32::MAX / 2 || self.deleted_offsets > u32::MAX / 2 {
            // Don't let the deleted_* shifts grow indefinitely otherwise
            // they will end up overflowing, breaking everything.
            self.reset_shifts();
        }
    }
}
