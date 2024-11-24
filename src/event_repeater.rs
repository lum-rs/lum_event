use lum_libs::{
    tokio::{self, sync::Mutex, task::JoinHandle},
    uuid::Uuid,
};
use lum_log::error;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, Weak},
};
use thiserror::Error;

use super::Event;

#[derive(Debug, Error)]
pub enum AttachError {
    #[error("Tried to attach event {event_name} to EventRepeater {repeater_name} while it was uninitialized. Did you not use EventRepeater<T>::new()?")]
    NotInitialized {
        event_name: String,
        repeater_name: String,
    },

    #[error(
        "Tried to attach event {event_name} to EventRepeater {repeater_name}, which was already attached to it."
    )]
    AlreadyAttached {
        event_name: String,
        repeater_name: String,
    },
}

#[derive(Debug, Error)]
pub enum DetachError {
    #[error(
        "Tried to detach event {event_name} from EventRepeater {repeater_name}, which was not attached."
    )]
    NotAttached {
        event_name: String,
        repeater_name: String,
    },
}

#[derive(Error)]
pub enum CloseError<T>
where
    T: Clone + Send + 'static,
{
    #[error("EventRepeater still has attached events. Detach all events before closing.")]
    AttachedEvents(EventRepeater<T>),
}

pub struct EventRepeater<T>
where
    T: Clone + Send + 'static,
{
    pub event: Event<T>,
    weak: OnceLock<Weak<Mutex<Self>>>,
    subscriptions: Mutex<HashMap<Uuid, (Uuid, JoinHandle<()>)>>,
}

unsafe impl<T> Send for EventRepeater<T> where T: Clone + Send + 'static {}
unsafe impl<T> Sync for EventRepeater<T> where T: Clone + Send + 'static {}

impl<T> EventRepeater<T>
where
    T: Clone + Send + 'static,
{
    pub async fn new<IntoString: Into<String>>(name: IntoString) -> Arc<Mutex<Self>> {
        let event = Event::new(name);
        let event_repeater = Self {
            weak: OnceLock::new(),
            event,
            subscriptions: Mutex::new(HashMap::new()),
        };

        let arc = Arc::new(Mutex::new(event_repeater));
        let weak = Arc::downgrade(&arc);

        let result = arc.lock().await.weak.set(weak);
        if result.is_err() {
            error!("Failed to set EventRepeater {}'s Weak self-reference because it was already set. This should never happen. Panicking to prevent further undefined behavior.", arc.lock().await.event.name);
            unreachable!(
                "Unable to set EventRepeater {}'s Weak self-reference because it was already set.",
                arc.lock().await.event.name
            );
        }

        arc
    }

    pub async fn subscription_count(&self) -> usize {
        self.subscriptions.lock().await.len()
    }

    pub async fn attach(&self, event: &mut Event<T>, buffer: usize) -> Result<(), AttachError> {
        let weak = match self.weak.get() {
            Some(weak) => weak,
            None => {
                return Err(AttachError::NotInitialized {
                    event_name: event.name.clone(),
                    repeater_name: self.event.name.clone(),
                })
            }
        };

        // This can't fail because the Arc is guaranteed to be valid as long as &self is valid.
        let arc = match weak.upgrade() {
            Some(arc) => arc,
            None => {
                error!("EventRepeater {}'s Weak self-reference could not be upgraded to Arc while attaching event {}. This should never happen. Panicking to prevent further undefined behavior.", self.event.name, event.name);
                unreachable!("EventRepeater {}'s Weak self-reference could not be upgraded to Arc while attaching event {}.", self.event.name, event.name);
            }
        };

        let mut subscriptions = self.subscriptions.lock().await;
        if subscriptions.contains_key(&event.uuid) {
            return Err(AttachError::AlreadyAttached {
                event_name: event.name.clone(),
                repeater_name: self.event.name.clone(),
            });
        }

        let (uuid, mut receiver) = event.subscribe_channel(&self.event.name, buffer, true, true);
        let join_handle = tokio::spawn(async move {
            while let Some(value) = receiver.recv().await {
                let _ = arc.lock().await.event.dispatch(value).await;
            }
        });
        subscriptions.insert(event.uuid, (uuid, join_handle));

        Ok(())
    }

    pub async fn detach(&self, event: &Event<T>) -> Result<(), DetachError> {
        let mut subscriptions = self.subscriptions.lock().await;

        let subscription = match subscriptions.remove(&event.uuid) {
            Some(subscription) => subscription,
            None => {
                return Err(DetachError::NotAttached {
                    event_name: event.name.clone(),
                    repeater_name: self.event.name.clone(),
                })
            }
        };
        subscription.1.abort();

        Ok(())
    }

    pub async fn close(self) -> Result<(), CloseError<T>> {
        let subscription_count = self.subscription_count().await;

        if subscription_count > 0 {
            return Err(CloseError::AttachedEvents(self));
        }

        Ok(())
    }
}

impl<T> AsRef<Event<T>> for EventRepeater<T>
where
    T: Clone + Send + 'static,
{
    fn as_ref(&self) -> &Event<T> {
        &self.event
    }
}
