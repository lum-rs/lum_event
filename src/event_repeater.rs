use lum_libs::{
    dashmap::DashMap,
    parking_lot::Mutex,
    tokio::{
        spawn,
        sync::mpsc::{Receiver, error::TryRecvError},
        task::JoinHandle,
        time,
    },
    uuid::Uuid,
};
use lum_log::{error, log::warn};
use std::{
    fmt::{self, Display, Formatter},
    sync::{Arc, Weak},
    time::Duration,
};
use thiserror::Error;

use super::Event;

struct Subscription<T: Clone + Send> {
    event: Weak<Event<T>>,
    subscriber_uuid: Uuid,
    receiver: Receiver<T>,
    log: bool,
}

#[derive(Debug, Error)]
pub enum AttachError {
    #[error(
        "Tried to attach EventRepeater {event_repeater_name} to Event {event_name}, which it was already attached to"
    )]
    AlreadyAttached {
        event_repeater_name: String,
        event_name: String,
    },
}

#[derive(Debug, Error)]
pub enum DetachError {
    #[error(
        "Tried to detach EventRepeater {event_repeater_name} from Event {event_name}, which it was not attached to because it was uninitialized"
    )]
    NotAttached {
        event_repeater_name: String,
        event_name: String,
    },
}

pub struct EventRepeater<T: Clone + Send + 'static> {
    pub event: Arc<Event<T>>,

    attachments: Arc<DashMap<Uuid, Subscription<T>>>,
    receive_dispatch_loop: Mutex<Option<JoinHandle<()>>>,
}

impl<T: Clone + Send + 'static> EventRepeater<T> {
    pub fn new(name: impl Into<String>) -> Self {
        let event = Event::new(name);

        Self {
            event: Arc::new(event),
            attachments: Arc::new(DashMap::new()),
            receive_dispatch_loop: Mutex::new(None),
        }
    }

    pub fn name(&self) -> &str {
        &self.event.name
    }

    pub fn attachment_count(&self) -> usize {
        self.attachments.len()
    }

    pub fn attach(
        &self,
        event: Arc<Event<T>>,
        buffer: usize,
        log: bool,
    ) -> Result<(), AttachError> {
        if self.attachments.contains_key(&event.uuid) {
            let event_repeater_name = self.name().to_string();
            let event_name = event.name.clone();

            return Err(AttachError::AlreadyAttached {
                event_repeater_name,
                event_name,
            });
        }

        let (subscriber_uuid, receiver) =
            event.subscribe_channel(&self.event.name, buffer, log, true); // we always want the repeater to be removed when the channel is closed

        let event_weak = Arc::downgrade(&event);
        let attachment = Subscription {
            event: event_weak,
            subscriber_uuid,
            receiver,
            log,
        };

        self.attachments.insert(event.uuid, attachment);
        self.trigger_receive_dispatch_loop();

        Ok(())
    }

    pub fn detach(&self, event: &Event<T>) -> Result<(), DetachError> {
        let event_repeater_name = self.name().to_string();
        let event_uuid = &event.uuid;
        let event_name = &event.name;
        let attachments = self.attachments.clone();
        let result = do_detach(
            &event_repeater_name,
            event_uuid,
            Some(event_name),
            attachments,
        );

        if result.is_ok() {
            self.trigger_receive_dispatch_loop();
        }

        result
    }

    pub fn close(self) {
        {
            let mut receive_loop = self.receive_dispatch_loop.lock();
            if let Some(handle) = receive_loop.as_ref() {
                handle.abort();
                *receive_loop = None;
            }
        }

        let uuids_to_remove = self
            .attachments
            .iter()
            .map(|entry| *entry.key())
            .collect::<Vec<_>>();

        for uuid in uuids_to_remove.into_iter() {
            let _ = do_detach(&self.event.name, &uuid, None, self.attachments.clone());
        }
    }

    // Goal: Make sure loop is stopped if no attachments, make sure loop is running otherwise
    fn trigger_receive_dispatch_loop(&self) {
        let mut receive_loop = self.receive_dispatch_loop.lock();

        if self.attachments.is_empty()
            && let Some(handle) = receive_loop.as_ref()
        {
            handle.abort();
            *receive_loop = None;
        } else if receive_loop.is_none() || receive_loop.as_ref().unwrap().is_finished() {
            let self_event = self.event.clone();
            let attachments = self.attachments.clone();
            let handle = spawn(async move {
                run_receive_dispatch_loop(self_event, attachments).await;
            });

            *receive_loop = Some(handle);
        }
    }
}

async fn run_receive_dispatch_loop<T: Clone + Send + 'static>(
    self_event: Arc<Event<T>>,
    attachments: Arc<DashMap<Uuid, Subscription<T>>>,
) {
    let event_repeater_name = self_event.name.clone();

    loop {
        let mut data_to_dispatch = Vec::new();
        let mut attachments_to_remove = Vec::new();
        for mut entry in attachments.iter_mut() {
            let event_uuid = *entry.key();
            let attachment = entry.value_mut();

            // Drain all available messages from the attachment's channel
            loop {
                let data = match attachment.receiver.try_recv() {
                    Ok(data) => data,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if attachment.log {
                            warn!(
                                "EventRepeater {}'s attachment {} closed its channel. It will be unregistered.",
                                event_repeater_name, attachment.subscriber_uuid
                            );
                        }
                        attachments_to_remove.push(event_uuid);
                        break;
                    }
                };

                // We hold back dispatching until we've checked all attachments to avoid
                // holding DashMap's lock across an await point
                data_to_dispatch.push(data);
            }
        }

        let should_yield = data_to_dispatch.is_empty();
        for data in data_to_dispatch.into_iter() {
            // Error-handling (logging + removing) is done on a per-attachment basis, so we do not need to handle it here
            let _ = self_event.dispatch(data).await;
        }

        for uuid in attachments_to_remove.into_iter() {
            let _ = do_detach(&event_repeater_name, &uuid, None, attachments.clone());
        }

        if attachments.is_empty() {
            break;
        }

        // We don't just yield here to avoid hugging the CPU when there's nothing else to do
        if should_yield {
            time::sleep(Duration::from_millis(1)).await;
        }
    }
}

fn do_detach<T: Clone + Send + 'static>(
    event_repeater_name: &str,
    event_uuid: &Uuid,
    event_name: Option<&str>,
    attachments: Arc<DashMap<Uuid, Subscription<T>>>,
) -> Result<(), DetachError> {
    let attachment = match attachments.remove(event_uuid) {
        Some((_, attachment)) => attachment,
        None => {
            let event_name = match event_name {
                Some(name) => name.to_string(),
                None => event_uuid.to_string(),
            };
            let event_repeater_name = event_repeater_name.to_string();

            return Err(DetachError::NotAttached {
                event_repeater_name,
                event_name,
            });
        }
    };

    if let Some(event) = attachment.event.upgrade() {
        let removed = event.unsubscribe(attachment.subscriber_uuid);
        if !removed && attachment.log {
            warn!(
                "EventRepeater {} tried to detach from event {} but the attachment was not registered as a subscriber anymore. It must have been removed already some other way.",
                event_repeater_name, event.name
            );
        }
    } else if attachment.log {
        let event_name = match event_name {
            Some(name) => name.to_string(),
            None => event_uuid.to_string(),
        };
        warn!(
            "EventRepeater {} tried to detach from event {} but the event has already been dropped. The attachment will be dropped.",
            event_repeater_name, event_name
        );
    }

    Ok(()) //attachment is dropped here, closing the receiver channel
}

impl<T: Clone + Send + 'static> PartialEq for EventRepeater<T> {
    fn eq(&self, other: &Self) -> bool {
        self.event == other.event
    }
}

impl<T: Clone + Send + 'static> PartialEq<Uuid> for EventRepeater<T> {
    fn eq(&self, other: &Uuid) -> bool {
        self.event.uuid == *other
    }
}

impl<T: Clone + Send + 'static> Eq for EventRepeater<T> {}

impl<T: Clone + Send + 'static> AsRef<Event<T>> for EventRepeater<T> {
    fn as_ref(&self) -> &Event<T> {
        self.event.as_ref()
    }
}

//TODO: Use async Drop when Rust supports it
impl<T: Clone + Send + 'static> Drop for EventRepeater<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.receive_dispatch_loop.get_mut().take() {
            handle.abort();
        }
    }
}

impl<T: Clone + Send + 'static> Display for EventRepeater<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let sub_count = self.attachment_count();
        let sub_word = if sub_count == 1 {
            "subscription"
        } else {
            "subscriptions"
        };

        write!(
            f,
            "EventRepeater {} ({} {})",
            self.event.name, sub_count, sub_word
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lum_libs::tokio::{self};

    const REPEATER_NAME: &str = "test_repeater";
    const EVENT_NAME: &str = "test_event";
    const RECEIVER_NAME: &str = "test_receiver";
    const BUFFER_SIZE: usize = 10;
    const LOG_ON_ERROR: bool = true;
    const DATA: u16 = 3120;

    fn get_receive_loop_status<T: Clone + Send + 'static>(
        event_repeater: &EventRepeater<T>,
    ) -> bool {
        let lock = event_repeater.receive_dispatch_loop.lock();
        match lock.as_ref() {
            Some(handle) => !handle.is_finished(),
            None => false,
        }
    }

    #[tokio::test]
    async fn display() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (0 subscriptions)", REPEATER_NAME)
        );

        let event1 = Event::new(EVENT_NAME);
        let event1 = Arc::new(event1);
        event_repeater
            .attach(event1.clone(), BUFFER_SIZE, LOG_ON_ERROR)
            .unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (1 subscription)", REPEATER_NAME)
        );

        let event2 = Event::new(EVENT_NAME);
        let event2 = Arc::new(event2);
        event_repeater
            .attach(event2.clone(), BUFFER_SIZE, LOG_ON_ERROR)
            .unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (2 subscriptions)", REPEATER_NAME)
        );

        event_repeater.detach(&event2).unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (1 subscription)", REPEATER_NAME)
        );

        event_repeater.detach(&event1).unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (0 subscriptions)", REPEATER_NAME)
        );
    }

    #[tokio::test]
    async fn receive_loop_management() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        assert!(!get_receive_loop_status(&event_repeater));

        let event1 = Event::new(EVENT_NAME);
        let event1 = Arc::new(event1);
        event_repeater
            .attach(event1.clone(), BUFFER_SIZE, LOG_ON_ERROR)
            .unwrap();
        assert!(get_receive_loop_status(&event_repeater));

        let event2 = Event::new(EVENT_NAME);
        let event2 = Arc::new(event2);
        event_repeater
            .attach(event2.clone(), BUFFER_SIZE, LOG_ON_ERROR)
            .unwrap();
        assert!(get_receive_loop_status(&event_repeater));

        event_repeater.detach(&event1).unwrap();
        assert!(get_receive_loop_status(&event_repeater));

        event_repeater.detach(&event2).unwrap();
        assert!(!get_receive_loop_status(&event_repeater));
    }

    #[tokio::test]
    async fn subscribe_and_unsubscribe_event() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1 = Arc::new(event1);
        event_repeater
            .attach(event1.clone(), BUFFER_SIZE, LOG_ON_ERROR)
            .unwrap();

        assert_eq!(event_repeater.attachment_count(), 1);
        assert_eq!(event1.subscriber_count(), 1);
        assert_eq!(Arc::strong_count(&event1), 1); // Repeater should downgrade to Weak

        event_repeater.detach(&event1).unwrap();
        assert_eq!(event_repeater.attachment_count(), 0);
        assert_eq!(event1.subscriber_count(), 0);
        assert_eq!(Arc::strong_count(&event1), 1);
    }

    #[tokio::test]
    async fn remove_closed_attachments() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1 = Arc::new(event1);

        event_repeater
            .attach(event1.clone(), BUFFER_SIZE, LOG_ON_ERROR) //should still remove, even with remove_on_error = false
            .unwrap();

        drop(event1);
        time::sleep(Duration::from_millis(10)).await; //Give some time for the receive loop to process the closed channel
        assert_eq!(event_repeater.attachment_count(), 0);
        assert!(!get_receive_loop_status(&event_repeater));
    }

    #[tokio::test]
    async fn remove_from_events_on_close() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1 = Arc::new(event1);

        event_repeater
            .attach(event1.clone(), BUFFER_SIZE, LOG_ON_ERROR)
            .unwrap();
        assert_eq!(event1.subscriber_count(), 1);

        event_repeater.close();
        assert_eq!(event1.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn remove_from_events_on_drop() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1 = Arc::new(event1);

        event_repeater
            .attach(event1.clone(), BUFFER_SIZE, LOG_ON_ERROR)
            .unwrap();
        assert_eq!(event1.subscriber_count(), 1);

        drop(event_repeater);
        assert_eq!(event1.subscriber_count(), 1); // event did not check for closed channels yet

        time::sleep(Duration::from_millis(10)).await; //Give some time for the receive loop to be aborted

        let result = event1.dispatch(()).await; // trigger the event to make the repeater notice the closed channel
        assert!(result.is_err());
        assert_eq!(event1.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn repeat_data() {
        let event_repeater = EventRepeater::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1 = Arc::new(event1);

        event_repeater
            .attach(event1.clone(), BUFFER_SIZE, LOG_ON_ERROR)
            .unwrap();

        let mut receiver = event_repeater
            .event
            .subscribe_channel(RECEIVER_NAME, BUFFER_SIZE, LOG_ON_ERROR, true)
            .1;

        event1.dispatch(DATA).await.unwrap();
        let received_data = receiver.recv().await.unwrap();
        assert_eq!(received_data, DATA);
    }
}
