use std::{
    any::type_name,
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
};

use lum_boxtypes::{BoxedError, PinnedBoxedFutureResult};
use lum_libs::{
    dashmap::DashMap,
    tokio::sync::mpsc::{Receiver, channel},
};
use lum_log::error;

use crate::{
    Subscriber,
    id::get_unique_id,
    subscriber::{Callback, DispatchError},
};

pub struct Event<T: Clone + Send> {
    pub name: String,
    pub id: u64,

    subscribers: DashMap<u64, Subscriber<T>>,
}

impl<T: Clone + Send> Event<T> {
    pub fn new(name: impl Into<String>) -> Self {
        let id = get_unique_id();

        Self {
            name: name.into(),
            id,
            subscribers: DashMap::new(),
        }
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    pub fn subscribe_channel(
        &self,
        name: impl Into<String>,
        buffer: usize,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> (u64, Receiver<T>) {
        let (sender, receiver) = channel(buffer);

        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::Channel(sender),
        );

        let id = subscriber.id;
        self.subscribers.insert(id, subscriber);

        (id, receiver)
    }

    pub fn subscribe_async_closure(
        &self,
        name: impl Into<String>,
        closure: impl Fn(T) -> PinnedBoxedFutureResult<()> + Send + Sync + 'static,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> u64 {
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::AsyncClosure(Box::new(closure)),
        );

        let id = subscriber.id;
        self.subscribers.insert(id, subscriber);

        id
    }

    pub fn subscribe_closure(
        &self,
        name: impl Into<String>,
        closure: impl Fn(T) -> Result<(), BoxedError> + Send + Sync + 'static,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> u64 {
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::Closure(Box::new(closure)),
        );

        let id = subscriber.id;
        self.subscribers.insert(id, subscriber);

        id
    }

    pub fn unsubscribe(&self, id: u64) -> bool {
        let value = self.subscribers.remove(&id);
        value.is_some()
    }

    //TODO: Docs about cancelation safety. data can be dropped without reaching a channel.
    pub async fn dispatch(&self, data: T) -> Result<(), Vec<DispatchError<T>>> {
        let mut errors = Vec::new();
        let mut subscribers_to_remove = Vec::new();

        for ref_multi in self.subscribers.iter() {
            let id = *ref_multi.key();
            let subscriber = ref_multi.value();

            let data = data.clone();
            let result = subscriber.dispatch(data).await;
            if let Err(err) = result {
                //TODO: Remove log_on_error/remove_on_error -> provide closure for error handling?
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

                    subscribers_to_remove.push(id);
                }

                errors.push(err);
            }
        }

        for id in subscribers_to_remove.into_iter() {
            self.subscribers.remove(&id);
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(())
    }
}

impl<T: Clone + Send> PartialEq for Event<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<T: Clone + Send> Eq for Event<T> {}

impl<T: Clone + Send> Hash for Event<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T: Clone + Send> Debug for Event<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let sub_count = self.subscribers.len();

        f.debug_struct(type_name::<Self>())
            .field("id", &self.id)
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
