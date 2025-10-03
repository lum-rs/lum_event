use lum_libs::{
    dashmap::DashMap,
    tokio::{
        spawn,
        sync::{
            Mutex,
            mpsc::{Receiver, error::TryRecvError},
        },
        task::JoinHandle,
    },
    uuid::Uuid,
};
use lum_log::{error, log::warn};
use std::{
    fmt::{self, Display, Formatter},
    sync::Arc,
};
use thiserror::Error;

use super::Event;

struct Subscriber<T> {
    uuid: Uuid,
    name: String,
    receiver: Receiver<T>,
    log_on_error: bool,
    remove_on_error: bool,
}

#[derive(Debug, Error)]
pub enum AttachError {
    #[error(
        "Tried to attach event {event_name} to EventRepeater {repeater_name} while it was uninitialized"
    )]
    NotInitialized {
        event_name: String,
        repeater_name: String,
    },

    #[error(
        "Tried to attach event {event_name} to EventRepeater {repeater_name}, which was already attached"
    )]
    AlreadyAttached {
        event_name: String,
        repeater_name: String,
    },
}

#[derive(Debug, Error)]
pub enum DetachError {
    #[error(
        "Tried to detach event {event_name} from EventRepeater {event_repeater_name}, which was not attached"
    )]
    NotAttached {
        event_name: String,
        event_repeater_name: String,
    },
}

pub struct EventRepeater<T: Clone + Send + 'static> {
    pub event: Arc<Event<T>>,

    subscriptions: Arc<DashMap<Uuid, Subscriber<T>>>,
    receive_loop: Mutex<Option<JoinHandle<()>>>, //TODO: use parkinglot Mutex
}

impl<T: Clone + Send + 'static> EventRepeater<T> {
    pub fn new(name: impl Into<String>) -> Self {
        let event = Event::new(name);

        Self {
            event: Arc::new(event),
            subscriptions: Arc::new(DashMap::new()),
            receive_loop: Mutex::new(None),
        }
    }

    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    pub async fn attach(
        &self,
        event: &Event<T>,
        buffer: usize,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> Result<(), AttachError> {
        if self.subscriptions.contains_key(&event.uuid) {
            let event_name = event.name.clone();
            let repeater_name = self.event.name.clone();

            return Err(AttachError::AlreadyAttached {
                event_name,
                repeater_name,
            });
        }

        let (subscriber_uuid, receiver) = event
            .subscribe_channel(&self.event.name, buffer, log_on_error, remove_on_error)
            .await;

        let subscriber = Subscriber {
            uuid: subscriber_uuid,
            name: event.name.clone(),
            receiver,
            log_on_error,
            remove_on_error,
        };

        self.subscriptions.insert(event.uuid, subscriber);
        self.update_receive_loop().await;

        Ok(())
    }

    pub async fn detach(&self, event: &Event<T>) -> Result<(), DetachError> {
        let uuid = &event.uuid;
        let result = match self.subscriptions.remove(uuid) {
            Some(_) => Ok(()),
            None => {
                let event_name = event.name.clone();
                let event_repeater_name = self.event.name.clone();

                Err(DetachError::NotAttached {
                    event_name,
                    event_repeater_name,
                })
            }
        };

        self.update_receive_loop().await;

        result
    }

    pub async fn close(self) {
        {
            let mut receive_loop = self.receive_loop.lock().await;
            if let Some(handle) = receive_loop.as_ref() {
                handle.abort();
            }
            *receive_loop = None;
        }

        for mut entry in self.subscriptions.iter_mut() {
            let subscriber = entry.value_mut();
            subscriber.receiver.close();
        }
        self.subscriptions.clear();
    }

    async fn update_receive_loop(&self) {
        let mut receive_loop = self.receive_loop.lock().await;

        if self.subscriptions.is_empty() {
            if let Some(handle) = receive_loop.as_ref() {
                handle.abort();
            }

            *receive_loop = None;
        } else if receive_loop.is_none() {
            let event = self.event.clone();
            let subscriptions = self.subscriptions.clone();
            let handle = spawn(async move {
                Self::run_receive_loop(event, subscriptions).await;
            });

            *receive_loop = Some(handle);
        }
    }

    async fn run_receive_loop(
        event: Arc<Event<T>>,
        subscriptions: Arc<DashMap<Uuid, Subscriber<T>>>,
    ) {
        loop {
            let mut subscribers_to_remove = Vec::new();
            for mut entry in subscriptions.iter_mut() {
                let event_uuid = *entry.key();
                let subscriber = entry.value_mut();

                let data = match subscriber.receiver.try_recv() {
                    Ok(data) => data,
                    Err(TryRecvError::Empty) => continue,
                    Err(TryRecvError::Disconnected) => {
                        if subscriber.log_on_error {
                            warn!(
                                "EventRepeater {}'s subscriber {} closed its channel. It will be removed from the list of subscribers.",
                                event.name, subscriber.uuid
                            );
                        }
                        subscribers_to_remove.push(event_uuid);
                        continue;
                    }
                };

                let dispatch_result = event.dispatch(data).await;
                if let Err(errs) = dispatch_result {
                    if subscriber.log_on_error {
                        for err in errs.iter() {
                            error!(
                                "EventRepeater {} failed to dispatch data to subscriber {}: {}.",
                                event.name, subscriber.name, err
                            );
                        }
                    }

                    if subscriber.remove_on_error {
                        if subscriber.log_on_error {
                            error!(
                                "Subscriber {} will be unregistered from EventRepeater {}.",
                                subscriber.name, event.name
                            );
                        }

                        subscribers_to_remove.push(event_uuid);
                    }
                }
            }

            for uuid in subscribers_to_remove.into_iter() {
                subscriptions.remove(&uuid);
            }
        }
    }
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
        &self.event
    }
}

//TODO: Hook into Drop?

impl<T: Clone + Send + 'static> Display for EventRepeater<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let sub_count = self.subscription_count();
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
    use lum_libs::tokio;

    const TEST_REPEATER_NAME: &str = "test_repeater";
    const TEST_EVENT_NAME: &str = "test_event";

    #[tokio::test]
    async fn test_display() {
        let event_repeater = EventRepeater::<()>::new(TEST_REPEATER_NAME);
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (0 subscriptions)", TEST_REPEATER_NAME)
        );

        let event1 = Event::new(TEST_EVENT_NAME);
        event_repeater
            .attach(&event1, 10, true, true)
            .await
            .unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (1 subscription)", TEST_REPEATER_NAME)
        );

        let event2 = Event::new(TEST_EVENT_NAME);
        event_repeater
            .attach(&event2, 10, true, true)
            .await
            .unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (2 subscriptions)", TEST_REPEATER_NAME)
        );

        event_repeater.detach(&event2).await.unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (1 subscription)", TEST_REPEATER_NAME)
        );

        event_repeater.detach(&event1).await.unwrap();
        let display_str = event_repeater.to_string();
        assert_eq!(
            display_str,
            format!("EventRepeater {} (0 subscriptions)", TEST_REPEATER_NAME)
        );
    }
}
