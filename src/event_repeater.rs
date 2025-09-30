use lum_libs::{
    dashmap::DashMap,
    tokio::{self, task::JoinHandle},
    uuid::Uuid,
};
use lum_log::{error, error_unreachable};
use std::sync::{Arc, OnceLock, Weak};
use thiserror::Error;

use super::Event;

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
        "Tried to attach event {event_name} to EventRepeater {repeater_name}, which was already attached to it"
    )]
    AlreadyAttached {
        event_name: String,
        repeater_name: String,
    },
}

#[derive(Debug, Error)]
pub enum DetachError {
    #[error(
        "Tried to detach event {event_name} from EventRepeater {repeater_name}, which was not attached"
    )]
    NotAttached {
        event_name: String,
        repeater_name: String,
    },
}

#[derive(Error)]
pub enum CloseError<T: Clone + Send + 'static> {
    #[error(
        "EventRepeater {repeater_name} still has attached events. Detach all events before closing"
    )]
    AttachedEvents {
        repeater_name: String,
        repeater: EventRepeater<T>,
    },
}

//TODO: Use single event loop task instead of one per attached event
pub struct EventRepeater<T: Clone + Send + 'static> {
    pub event: Event<T>,

    weak: OnceLock<Weak<Self>>,
    subscriptions: DashMap<Uuid, (Uuid, JoinHandle<()>)>,
}

impl<T: Clone + Send + 'static> EventRepeater<T> {
    pub async fn new(name: impl Into<String>) -> Arc<Self> {
        let event = Event::new(name);
        let event_repeater = Self {
            weak: OnceLock::new(),
            event,
            subscriptions: DashMap::new(),
        };

        let arc = Arc::new(event_repeater);
        let weak = Arc::downgrade(&arc);

        let result = arc.weak.set(weak);
        if result.is_err() {
            error_unreachable!(
                "Failed to set EventRepeater {}'s Weak self-reference because it was already set. This should never happen. Panicking to prevent further undefined behavior.",
                arc.event.name
            );
        }

        arc
    }

    pub async fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    pub async fn attach(&self, event: &Event<T>, buffer: usize) -> Result<(), AttachError> {
        let weak = match self.weak.get() {
            Some(weak) => weak,
            None => {
                let event_name = event.name.clone();
                let repeater_name = self.event.name.clone();
                return Err(AttachError::NotInitialized {
                    event_name,
                    repeater_name,
                });
            }
        };

        // This can't fail because the Arc is guaranteed to be valid as long as &self is valid.
        let arc = match weak.upgrade() {
            Some(arc) => arc,
            None => {
                error_unreachable!(
                    "EventRepeater {}'s Weak self-reference could not be upgraded to Arc while attaching event {}. This should never happen. Panicking to prevent further undefined behavior.",
                    self.event.name,
                    event.name
                );
            }
        };

        if self.subscriptions.contains_key(&event.uuid) {
            let event_name = event.name.clone();
            let repeater_name = self.event.name.clone();

            return Err(AttachError::AlreadyAttached {
                event_name,
                repeater_name,
            });
        }

        let (subscriber_uuid, mut receiver) = event
            .subscribe_channel(&self.event.name, buffer, true, true)
            .await;

        let join_handle = tokio::spawn(async move {
            while let Some(value) = receiver.recv().await {
                let _ = arc.event.dispatch(value).await;
            }
        });

        self.subscriptions
            .insert(event.uuid, (subscriber_uuid, join_handle));

        Ok(())
    }

    pub async fn detach(&self, event: &Event<T>) -> Result<(), DetachError> {
        let subscription = match self.subscriptions.remove(&event.uuid) {
            Some(subscription) => subscription,
            None => {
                let event_name = event.name.clone();
                let repeater_name = self.event.name.clone();

                return Err(DetachError::NotAttached {
                    event_name,
                    repeater_name,
                });
            }
        };
        subscription.1.1.abort();

        Ok(())
    }

    pub async fn close(self) -> Result<(), CloseError<T>> {
        let subscription_count = self.subscription_count().await;

        if subscription_count > 0 {
            let repeater_name = self.event.name.clone();
            return Err(CloseError::AttachedEvents {
                repeater_name,
                repeater: self,
            });
        }

        Ok(())
    }
}

impl<T: Clone + Send + 'static> AsRef<Event<T>> for EventRepeater<T> {
    fn as_ref(&self) -> &Event<T> {
        &self.event
    }
}
