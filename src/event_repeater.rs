use lum_boxtypes::BoxedError;
use lum_libs::{
    dashmap::DashMap,
    tokio::{
        spawn,
        sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    },
};
use std::{
    fmt::{self, Display, Formatter},
    sync::Arc,
};
use thiserror::Error;

use crate::event::EventHandleError;

use super::{Event, event::EventHandle};

struct Attachment<T: Clone + Send> {
    event: EventHandle<T>,
    subscriber_id: u64,
}

#[derive(Debug, Error)]
pub enum AttachError {
    #[error("The EventHandle hit an error while attaching: {0}")]
    EventHandle(#[from] EventHandleError),

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
    #[error("The EventHandle hit an error while detaching: {0}")]
    EventHandle(#[from] EventHandleError),

    #[error(
        "Tried to detach EventRepeater {event_repeater_name} from Event {event_name}, which it was not attached to because it was uninitialized"
    )]
    NotAttached {
        event_repeater_name: String,
        event_name: String,
    },
}

#[derive(Debug, Error)]
pub enum ForwardingError {
    #[error("The EventRepeater {event_repeater_name} is no longer alive")]
    RepeaterDropped { event_repeater_name: String },
}

pub struct EventRepeater<T: Clone + Send + 'static> {
    pub event: Event<T>,

    attachments: Arc<DashMap<u64, Attachment<T>>>,
    alive: Arc<()>,
    event_queue_sender: UnboundedSender<T>,
}

impl<T: Clone + Send + 'static> EventRepeater<T> {
    pub fn new(name: impl Into<String>) -> Self {
        let event = Event::new(name);
        let event_handle = event.handle();

        let attachments = Arc::new(DashMap::new());
        let alive = Arc::new(());
        let (event_queue_sender, event_queue_receiver) = unbounded_channel::<T>();
        spawn(async move {
            run_forward_loop(event_handle, event_queue_receiver).await;
        });

        Self {
            event,
            attachments,
            event_queue_sender,
            alive,
        }
    }

    pub fn name(&self) -> &str {
        self.event.name()
    }

    pub fn attachment_count(&self) -> usize {
        self.cleanup_dropped_attachments();
        self.attachments.len()
    }

    pub fn attach(&self, event_handle: impl Into<EventHandle<T>>) -> Result<(), AttachError> {
        let event_handle = event_handle.into();

        event_handle.try_with(|event| {
            let event_id = event.id();
            let event_repeater_name = self.name().to_string();

            if self.attachments.contains_key(&event_id) {
                let event_name = event.name().to_string();

                return Err(AttachError::AlreadyAttached {
                    event_repeater_name,
                    event_name,
                });
            }

            let alive = Arc::downgrade(&self.alive);
            let event_queue_sender = self.event_queue_sender.clone();
            let subscriber_id = event.subscribe_closure(
                event_repeater_name.clone(),
                move |data: T| -> Result<(), BoxedError> {
                    if alive.upgrade().is_none() {
                        return Err(Box::new(ForwardingError::RepeaterDropped {
                            event_repeater_name: event_repeater_name.clone(),
                        }));
                    }

                    event_queue_sender.send(data).map_err(|_| {
                        Box::new(ForwardingError::RepeaterDropped {
                            event_repeater_name: event_repeater_name.clone(),
                        }) as BoxedError
                    })
                },
                false,
                true,
            );

            let attachment = Attachment {
                event: event_handle.clone(),
                subscriber_id,
            };

            self.attachments.insert(event_id, attachment);
            Ok(())
        })?
    }

    pub fn detach(&self, event_handle: impl Into<EventHandle<T>>) -> Result<(), DetachError> {
        let event_handle = event_handle.into();

        event_handle.try_with(|event| {
            let event_repeater_name = self.name().to_string();
            let attachments = self.attachments.clone();

            let attachment = match attachments.remove(&event.id()) {
                Some((_, attachment)) => attachment,
                None => {
                    let event_name = event.name().to_string();
                    return Err(DetachError::NotAttached {
                        event_repeater_name,
                        event_name,
                    });
                }
            };

            if attachment.event.is_dropped() {
                return Ok(());
            }

            let _ = attachment
                .event
                .try_with(|event| event.unsubscribe(attachment.subscriber_id));

            Ok(())
        })?
    }

    fn cleanup_dropped_attachments(&self) {
        let dropped: Vec<u64> = self
            .attachments
            .iter()
            .filter(|entry| entry.value().event.is_dropped())
            .map(|entry| *entry.key())
            .collect();

        for id in dropped {
            self.attachments.remove(&id);
        }
    }
}

async fn run_forward_loop<T: Clone + Send + 'static>(
    event_handle: EventHandle<T>,
    mut event_queue_receiver: UnboundedReceiver<T>,
) {
    while let Some(data) = event_queue_receiver.recv().await {
        if event_handle.dispatch(data).await.is_err() {
            return;
        }
    }
}

impl<T: Clone + Send + 'static> PartialEq for EventRepeater<T> {
    fn eq(&self, other: &Self) -> bool {
        self.event.id() == other.event.id()
    }
}

impl<T: Clone + Send + 'static> PartialEq<u64> for EventRepeater<T> {
    fn eq(&self, other: &u64) -> bool {
        self.event.id() == *other
    }
}

impl<T: Clone + Send + 'static> Eq for EventRepeater<T> {}

impl<T: Clone + Send + 'static> AsRef<Event<T>> for EventRepeater<T> {
    fn as_ref(&self) -> &Event<T> {
        &self.event
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
            self.event.name(),
            sub_count,
            sub_word
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use lum_libs::tokio::{self, time::sleep};

    const REPEATER_NAME: &str = "test_repeater";
    const EVENT_NAME: &str = "test_event";
    const RECEIVER_NAME: &str = "test_receiver";
    const DATA: u16 = 3120;

    #[tokio::test]
    async fn display() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (0 subscriptions)", REPEATER_NAME)
        );

        let event1 = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();
        event_repeater.attach(event1_handle.clone()).unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (1 subscription)", REPEATER_NAME)
        );

        let event2 = Event::new(EVENT_NAME);
        let event2_handle = event2.handle();
        event_repeater.attach(event2_handle.clone()).unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (2 subscriptions)", REPEATER_NAME)
        );

        event_repeater.detach(event2_handle).unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (1 subscription)", REPEATER_NAME)
        );

        event_repeater.detach(event1_handle).unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (0 subscriptions)", REPEATER_NAME)
        );
    }

    #[tokio::test]
    async fn attachment_lifecycle() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        assert_eq!(event_repeater.attachment_count(), 0);

        let event1 = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();
        event_repeater.attach(event1_handle.clone()).unwrap();
        assert_eq!(event_repeater.attachment_count(), 1);

        let event2 = Event::new(EVENT_NAME);
        let event2_handle = event2.handle();
        event_repeater.attach(event2_handle.clone()).unwrap();
        assert_eq!(event_repeater.attachment_count(), 2);

        event_repeater.detach(event1_handle).unwrap();
        assert_eq!(event_repeater.attachment_count(), 1);

        event_repeater.detach(event2_handle).unwrap();
        assert_eq!(event_repeater.attachment_count(), 0);
    }

    #[tokio::test]
    async fn subscribe_and_unsubscribe_event() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();
        event_repeater.attach(event1_handle.clone()).unwrap();

        assert_eq!(event_repeater.attachment_count(), 1);
        assert_eq!(event1.subscriber_count(), 1);

        event_repeater.detach(event1_handle).unwrap();
        assert_eq!(event_repeater.attachment_count(), 0);
        assert_eq!(event1.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn remove_closed_attachments() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();

        event_repeater.attach(event1_handle).unwrap();
        drop(event1);

        let attachment_count = event_repeater.attachment_count();
        assert_eq!(attachment_count, 0);
    }

    #[tokio::test]
    async fn remove_from_events_on_drop() {
        let event_repeater = EventRepeater::<()>::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();

        event_repeater.attach(event1_handle).unwrap();
        assert_eq!(event1.subscriber_count(), 1);

        drop(event_repeater);
        assert_eq!(event1.subscriber_count(), 1); // forwarding closure not yet removed

        let result = event1.dispatch(()).await; // triggers forwarding closure, which errors and self-removes
        assert!(result.is_err());
        assert_eq!(event1.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn stop_forward_loop_on_drop() {
        let (event_queue_sender, event_queue_receiver) = unbounded_channel::<()>();
        let event = Event::new("stop_test");
        let event_handle = event.handle();
        let task_handle = spawn(async move {
            run_forward_loop(event_handle, event_queue_receiver).await;
        });

        drop(event_queue_sender); // Closes the channel, which will cause the forward loop to exit
        sleep(Duration::from_millis(50)).await;

        assert!(task_handle.is_finished());
    }

    #[tokio::test]
    async fn repeat_data() {
        let event_repeater = EventRepeater::new(REPEATER_NAME);
        let event1 = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();

        event_repeater.attach(event1_handle).unwrap();

        let mut receiver = event_repeater
            .event
            .subscribe_channel(RECEIVER_NAME, 1, false, true)
            .1;

        event1.dispatch(DATA).await.unwrap();
        let received_data = receiver.recv().await.unwrap();
        assert_eq!(received_data, DATA);
    }
}
