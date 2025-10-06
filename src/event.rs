use std::{
    any::type_name,
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
};

use lum_boxtypes::{BoxedError, PinnedBoxedFutureResult};
use lum_libs::{
    dashmap::DashMap,
    tokio::sync::mpsc::{Receiver, channel},
    uuid::Uuid,
};
use lum_log::error;

use crate::{
    Subscriber,
    subscriber::{Callback, DispatchError},
};

pub struct Event<T: Clone + Send> {
    pub name: String,
    pub uuid: Uuid,

    subscribers: DashMap<Uuid, Subscriber<T>>,
}

impl<T: Clone + Send> Event<T> {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            uuid: Uuid::new_v4(),
            subscribers: DashMap::new(),
        }
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    pub async fn subscribe_channel(
        &self,
        name: impl Into<String>,
        buffer: usize,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> (Uuid, Receiver<T>) {
        let (sender, receiver) = channel(buffer);
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::Channel(sender),
        );
        let uuid = subscriber.uuid;
        self.subscribers.insert(uuid, subscriber);

        (uuid, receiver)
    }

    pub async fn subscribe_async_closure(
        &self,
        name: impl Into<String>,
        closure: impl Fn(T) -> PinnedBoxedFutureResult<()> + Send + Sync + 'static,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> Uuid {
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::AsyncClosure(Box::new(closure)),
        );
        let uuid = subscriber.uuid;
        self.subscribers.insert(uuid, subscriber);

        uuid
    }

    pub async fn subscribe_closure(
        &self,
        name: impl Into<String>,
        closure: impl Fn(T) -> Result<(), BoxedError> + Send + Sync + 'static,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> Uuid {
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::Closure(Box::new(closure)),
        );
        let uuid = subscriber.uuid;
        self.subscribers.insert(uuid, subscriber);

        uuid
    }

    pub async fn unsubscribe(&self, uuid: impl AsRef<Uuid>) -> bool {
        let uuid = uuid.as_ref();
        let value = self.subscribers.remove(uuid);
        value.is_some()
    }

    pub async fn dispatch(&self, data: T) -> Result<(), Vec<DispatchError<T>>> {
        let mut errors = Vec::new();
        let mut subscribers_to_remove = Vec::new();

        for ref_multi in self.subscribers.iter() {
            let uuid = *ref_multi.key();
            let subscriber = ref_multi.value();

            let data = data.clone();
            let result = subscriber.dispatch(data).await;
            if let Err(err) = result {
                if subscriber.log_on_error {
                    error!(
                        "Event \"{}\" failed to dispatch data to subscriber \"{}\": {}.",
                        self.name, subscriber.name, err
                    );
                }

                if subscriber.remove_on_error {
                    if subscriber.log_on_error {
                        error!(
                            "Event \"{}\" will remove subscriber \"{}\" due to the error.",
                            self.name, subscriber.name
                        );
                    }

                    subscribers_to_remove.push(uuid);
                }

                errors.push(err);
            }
        }

        for uuid in subscribers_to_remove.into_iter() {
            self.subscribers.remove(&uuid);
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(())
    }
}

impl<T: Clone + Send> PartialEq for Event<T> {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}

impl<T: Clone + Send> PartialEq<Uuid> for Event<T> {
    fn eq(&self, other: &Uuid) -> bool {
        self.uuid == *other
    }
}

impl<T: Clone + Send> Eq for Event<T> {}

impl<T: Clone + Send> Hash for Event<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uuid.hash(state);
    }
}

impl<T: Clone + Send> Debug for Event<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let sub_count = self.subscribers.len();

        f.debug_struct(type_name::<Self>())
            .field("uuid", &self.uuid)
            .field("name", &self.name)
            .field("subscribers", &sub_count)
            .finish()
    }
}

impl<T: Clone + Send> Display for Event<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let sub_count = self.subscribers.len();
        let sub_word = if sub_count == 1 {
            "subscriber"
        } else {
            "subscribers"
        };

        write!(f, "Event {} ({} {})", self.name, sub_count, sub_word)
    }
}
