use std::{
    any::type_name,
    fmt::{self, Debug, Display, Formatter},
};

use lum_boxtypes::{BoxedError, PinnedBoxedFutureResult};
use lum_libs::{
    tokio::sync::{
        mpsc::{channel, Receiver},
        Mutex,
    },
    uuid::Uuid,
};
use lum_log::error;

use crate::{
    subscriber::{Callback, DispatchError},
    Subscriber,
};

pub struct Event<T>
where
    T: Clone + Send,
{
    pub name: String,

    pub uuid: Uuid,
    subscribers: Mutex<Vec<Subscriber<T>>>,
}

impl<T> Event<T>
where
    T: Clone + Send,
{
    pub fn new<IntoString: Into<String>>(name: IntoString) -> Self {
        Self {
            name: name.into(),
            uuid: Uuid::new_v4(),
            subscribers: Mutex::new(Vec::new()),
        }
    }

    pub async fn subscriber_count(&self) -> usize {
        let subscribers = self.subscribers.lock().await;
        subscribers.len()
    }

    pub async fn subscribe_channel<IntoString: Into<String>>(
        &self,
        name: IntoString,
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

        let mut subscribers = self.subscribers.lock().await;
        subscribers.push(subscriber);

        (uuid, receiver)
    }

    pub async fn subscribe_async_closure<IntoString: Into<String>, ClosureFn>(
        &self,
        name: IntoString,
        closure: ClosureFn,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> Uuid
    where
        ClosureFn: Fn(T) -> PinnedBoxedFutureResult<()> + Send + Sync + 'static,
    {
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::AsyncClosure(Box::new(closure)),
        );
        let uuid = subscriber.uuid;

        let mut subscribers = self.subscribers.lock().await;
        subscribers.push(subscriber);

        uuid
    }

    pub async fn subscribe_closure<IntoString: Into<String>, ClosureFn>(
        &self,
        name: IntoString,
        closure: ClosureFn,
        log_on_error: bool,
        remove_on_error: bool,
    ) -> Uuid
    where
        ClosureFn: Fn(T) -> Result<(), BoxedError> + Send + Sync + 'static,
    {
        let subscriber = Subscriber::new(
            name,
            log_on_error,
            remove_on_error,
            Callback::Closure(Box::new(closure)),
        );
        let uuid = subscriber.uuid;

        let mut subscribers = self.subscribers.lock().await;
        subscribers.push(subscriber);

        uuid
    }

    pub async fn unsubscribe<UuidRef: AsRef<Uuid>>(&self, uuid: &UuidRef) -> bool {
        let uuid = uuid.as_ref();
        let mut subscribers = self.subscribers.lock().await;

        let index = subscribers
            .iter()
            .map(|subscriber| &subscriber.uuid)
            .position(|subscriber_uuid| *subscriber_uuid == *uuid);

        match index {
            Some(index) => {
                subscribers.remove(index);
                true
            }
            None => false,
        }
    }

    pub async fn dispatch(&self, data: T) -> Result<(), Vec<DispatchError<T>>> {
        let mut subscribers = self.subscribers.lock().await;

        let mut errors = Vec::new();
        let mut subscribers_to_remove = Vec::new();

        for (index, subscriber) in subscribers.iter().enumerate() {
            let data = data.clone();

            let result = subscriber.dispatch(data).await;
            if let Err(err) = result {
                if subscriber.log_on_error {
                    error!(
                        "Event \"{}\" failed to dispatch data to subscriber {}: {}.",
                        self.name, subscriber.name, err
                    );
                }

                if subscriber.remove_on_error {
                    if subscriber.log_on_error {
                        error!("Subscriber will be unregistered from event.");
                    }

                    subscribers_to_remove.push(index);
                }

                errors.push(err);
            }
        }

        for index in subscribers_to_remove.into_iter().rev() {
            subscribers.remove(index);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl<T> PartialEq for Event<T>
where
    T: Clone + Send,
{
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}

impl<T> PartialEq<Uuid> for Event<T>
where
    T: Clone + Send,
{
    fn eq(&self, other: &Uuid) -> bool {
        self.uuid == *other
    }
}

impl<T> Eq for Event<T> where T: Clone + Send {}

impl<T> Debug for Event<T>
where
    T: Clone + Send,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("uuid", &self.uuid)
            .field("name", &self.name)
            .field("subscribers", &self.subscribers.blocking_lock().len())
            .finish()
    }
}

impl<T> Display for Event<T>
where
    T: Clone + Send,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Event {}", self.name)
    }
}
