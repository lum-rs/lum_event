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

struct Attachment {
    is_dropped: Box<dyn Fn() -> bool + Send + Sync + 'static>,
    do_unsubscribe: Box<dyn Fn() + Send + Sync + 'static>,
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

pub struct EventRepeater<IN: Clone + Send + 'static, OUT: Clone + Send + 'static = IN> {
    pub event: Event<OUT>,

    attachments: Arc<DashMap<u64, Attachment>>,
    alive: Arc<()>,
    event_queue_sender: UnboundedSender<IN>,
}

impl<T: Clone + Send + 'static> EventRepeater<T, T> {
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_transform(name, |data| data)
    }
}

impl<IN: Clone + Send + 'static, OUT: Clone + Send + 'static> EventRepeater<IN, OUT> {
    pub fn new_with_transform(
        name: impl Into<String>,
        transform: impl Fn(IN) -> OUT + Send + 'static,
    ) -> Self {
        let event = Event::new(name);
        let event_handle = event.handle();

        let attachments = Arc::new(DashMap::new());
        let alive = Arc::new(());
        let (event_queue_sender, event_queue_receiver) = unbounded_channel::<IN>();
        spawn(async move {
            run_forward_loop(event_handle, event_queue_receiver, Box::new(transform)).await;
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

    pub fn attach(&self, event_handle: impl Into<EventHandle<IN>>) -> Result<(), AttachError> {
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
                move |data: IN| -> Result<(), BoxedError> {
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

            let handle_for_is_dropped = event_handle.clone();
            let handle_for_unsubscribe = event_handle.clone();
            let attachment = Attachment {
                is_dropped: Box::new(move || handle_for_is_dropped.is_dropped()),
                do_unsubscribe: Box::new(move || {
                    let _ =
                        handle_for_unsubscribe.try_with(|event| event.unsubscribe(subscriber_id));
                }),
            };

            self.attachments.insert(event_id, attachment);
            Ok(())
        })?
    }

    pub fn attach_with_transform<S: Clone + Send + 'static>(
        &self,
        event_handle: impl Into<EventHandle<S>>,
        transform: impl Fn(S) -> IN + Send + Sync + 'static,
    ) -> Result<(), AttachError> {
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
                move |data: S| -> Result<(), BoxedError> {
                    if alive.upgrade().is_none() {
                        return Err(Box::new(ForwardingError::RepeaterDropped {
                            event_repeater_name: event_repeater_name.clone(),
                        }));
                    }

                    let transformed = transform(data);
                    event_queue_sender.send(transformed).map_err(|_| {
                        Box::new(ForwardingError::RepeaterDropped {
                            event_repeater_name: event_repeater_name.clone(),
                        }) as BoxedError
                    })
                },
                false,
                true,
            );

            let handle_for_is_dropped = event_handle.clone();
            let handle_for_unsubscribe = event_handle.clone();
            let attachment = Attachment {
                is_dropped: Box::new(move || handle_for_is_dropped.is_dropped()),
                do_unsubscribe: Box::new(move || {
                    let _ =
                        handle_for_unsubscribe.try_with(|event| event.unsubscribe(subscriber_id));
                }),
            };

            self.attachments.insert(event_id, attachment);
            Ok(())
        })?
    }

    pub fn detach<S: Clone + Send + 'static>(
        &self,
        event_handle: impl Into<EventHandle<S>>,
    ) -> Result<(), DetachError> {
        let event_handle = event_handle.into();

        event_handle.try_with(|event| {
            let event_repeater_name = self.name().to_string();

            let attachment = match self.attachments.remove(&event.id()) {
                Some((_, attachment)) => attachment,
                None => {
                    let event_name = event.name().to_string();
                    return Err(DetachError::NotAttached {
                        event_repeater_name,
                        event_name,
                    });
                }
            };

            if (attachment.is_dropped)() {
                return Ok(());
            }

            (attachment.do_unsubscribe)();
            Ok(())
        })?
    }

    fn cleanup_dropped_attachments(&self) {
        let dropped: Vec<u64> = self
            .attachments
            .iter()
            .filter(|entry| (entry.value().is_dropped)())
            .map(|entry| *entry.key())
            .collect();

        for id in dropped {
            self.attachments.remove(&id);
        }
    }
}

async fn run_forward_loop<IN: Clone + Send + 'static, OUT: Clone + Send + 'static>(
    event_handle: EventHandle<OUT>,
    mut event_queue_receiver: UnboundedReceiver<IN>,
    transform: Box<dyn Fn(IN) -> OUT + Send + 'static>,
) {
    while let Some(data) = event_queue_receiver.recv().await {
        let out = transform(data);
        if event_handle.dispatch(out).await.is_err() {
            return;
        }
    }
}

impl<IN: Clone + Send + 'static, OUT: Clone + Send + 'static> PartialEq for EventRepeater<IN, OUT> {
    fn eq(&self, other: &Self) -> bool {
        self.event.id() == other.event.id()
    }
}

impl<IN: Clone + Send + 'static, OUT: Clone + Send + 'static> PartialEq<u64>
    for EventRepeater<IN, OUT>
{
    fn eq(&self, other: &u64) -> bool {
        self.event.id() == *other
    }
}

impl<IN: Clone + Send + 'static, OUT: Clone + Send + 'static> Eq for EventRepeater<IN, OUT> {}

impl<IN: Clone + Send + 'static, OUT: Clone + Send + 'static> AsRef<Event<OUT>>
    for EventRepeater<IN, OUT>
{
    fn as_ref(&self) -> &Event<OUT> {
        &self.event
    }
}

impl<IN: Clone + Send + 'static, OUT: Clone + Send + 'static> Display for EventRepeater<IN, OUT> {
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
            run_forward_loop(event_handle, event_queue_receiver, Box::new(|x| x)).await;
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

    #[tokio::test]
    async fn repeat_data_with_attach_transform() {
        let event_repeater: EventRepeater<usize> = EventRepeater::new(REPEATER_NAME);
        let event1: Event<String> = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();

        event_repeater
            .attach_with_transform(event1_handle, |s| s.len())
            .unwrap();

        let mut receiver = event_repeater
            .event
            .subscribe_channel(RECEIVER_NAME, 1, false, true)
            .1;

        event1.dispatch("hello".to_string()).await.unwrap();
        let received_data = receiver.recv().await.unwrap();
        assert_eq!(received_data, 5);
    }

    #[tokio::test]
    async fn repeat_data_with_repeater_transform() {
        let event_repeater: EventRepeater<u16, String> =
            EventRepeater::new_with_transform(REPEATER_NAME, |n: u16| n.to_string());
        let event1: Event<u16> = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();

        event_repeater.attach(event1_handle).unwrap();

        let mut receiver = event_repeater
            .event
            .subscribe_channel(RECEIVER_NAME, 1, false, true)
            .1;

        event1.dispatch(DATA).await.unwrap();
        let received_data = receiver.recv().await.unwrap();
        assert_eq!(received_data, DATA.to_string());
    }

    #[tokio::test]
    async fn repeat_data_with_both_transforms() {
        let event_repeater: EventRepeater<usize, String> =
            EventRepeater::new_with_transform(REPEATER_NAME, |n: usize| n.to_string());
        let event1: Event<String> = Event::new(EVENT_NAME);

        event_repeater
            .attach_with_transform(event1.handle(), |s| s.len())
            .unwrap();

        let mut receiver = event_repeater
            .event
            .subscribe_channel(RECEIVER_NAME, 1, false, true)
            .1;

        event1.dispatch("hello".to_string()).await.unwrap();
        let received_data = receiver.recv().await.unwrap();
        assert_eq!(received_data, "5");
    }

    #[tokio::test]
    async fn attach_with_transform_lifecycle() {
        let event_repeater: EventRepeater<usize> = EventRepeater::new(REPEATER_NAME);
        let event1: Event<String> = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();

        assert_eq!(event_repeater.attachment_count(), 0);
        assert_eq!(event1.subscriber_count(), 0);

        event_repeater
            .attach_with_transform(event1_handle.clone(), |s| s.len())
            .unwrap();

        assert_eq!(event_repeater.attachment_count(), 1);
        assert_eq!(event1.subscriber_count(), 1);

        event_repeater.detach(event1_handle).unwrap();

        assert_eq!(event_repeater.attachment_count(), 0);
        assert_eq!(event1.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn remove_closed_attachments_with_transform() {
        let event_repeater: EventRepeater<usize> = EventRepeater::new(REPEATER_NAME);
        let event1: Event<String> = Event::new(EVENT_NAME);

        event_repeater
            .attach_with_transform(event1.handle(), |s| s.len())
            .unwrap();

        drop(event1);

        assert_eq!(event_repeater.attachment_count(), 0);
    }

    #[tokio::test]
    async fn remove_from_events_on_drop_with_transform() {
        let event_repeater: EventRepeater<usize> = EventRepeater::new(REPEATER_NAME);
        let event1: Event<String> = Event::new(EVENT_NAME);

        event_repeater
            .attach_with_transform(event1.handle(), |s: String| s.len())
            .unwrap();
        assert_eq!(event1.subscriber_count(), 1);

        drop(event_repeater);
        assert_eq!(event1.subscriber_count(), 1); // forwarding closure not yet removed

        let result = event1.dispatch("hello".to_string()).await; // triggers forwarding closure, which errors and self-removes
        assert!(result.is_err());
        assert_eq!(event1.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn attach_with_transform_already_attached() {
        let event_repeater: EventRepeater<usize> = EventRepeater::new(REPEATER_NAME);
        let event1: Event<String> = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();

        event_repeater
            .attach_with_transform(event1_handle.clone(), |s| s.len())
            .unwrap();

        let result = event_repeater.attach_with_transform(event1_handle, |s| s.len() + 1);

        assert!(matches!(result, Err(AttachError::AlreadyAttached { .. })));
    }

    #[tokio::test]
    async fn attach_and_attach_with_transform_already_attached() {
        let event_repeater: EventRepeater<()> = EventRepeater::new(REPEATER_NAME);
        let event1: Event<()> = Event::new(EVENT_NAME);
        let event1_handle = event1.handle();

        event_repeater.attach(event1_handle.clone()).unwrap();

        let result = event_repeater.attach_with_transform(event1_handle, |x: ()| x);

        assert!(matches!(result, Err(AttachError::AlreadyAttached { .. })));
    }
}
