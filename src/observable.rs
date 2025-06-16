use crate::{Event, subscriber::DispatchError};

#[derive(Debug)]
pub enum Result<T> {
    Unchanged,
    Changed(core::result::Result<(), Vec<DispatchError<T>>>),
}

#[derive(Debug)]
pub struct Observable<T>
where
    T: Clone + Send + PartialEq,
{
    pub on_change: Event<T>,

    value: T,
}

impl<T> Observable<T>
where
    T: Clone + Send + PartialEq,
{
    pub fn new(value: T, event_name: impl Into<String>) -> Self {
        Self {
            value,
            on_change: Event::new(event_name),
        }
    }

    pub fn get(&self) -> T {
        self.value.clone()
    }

    pub async fn set(&mut self, value: T) -> Result<T> {
        let current_value = self.value.clone();

        if current_value == value {
            return Result::Unchanged;
        }

        self.value = value.clone();
        let dispatch_result = self.on_change.dispatch(value).await;
        match dispatch_result {
            Ok(_) => Result::Changed(Ok(())),
            Err(errors) => Result::Changed(Err(errors)),
        }
    }
}

impl<T> AsRef<T> for Observable<T>
where
    T: Clone + Send + PartialEq,
{
    fn as_ref(&self) -> &T {
        &self.value
    }
}

impl AsRef<str> for Observable<&str> {
    fn as_ref(&self) -> &str {
        self.value
    }
}

impl<T> AsMut<T> for Observable<T>
where
    T: Clone + Send + PartialEq,
{
    fn as_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T> AsRef<Event<T>> for Observable<T>
where
    T: Clone + Send + PartialEq,
{
    fn as_ref(&self) -> &Event<T> {
        &self.on_change
    }
}

impl<T> AsMut<Event<T>> for Observable<T>
where
    T: Clone + Send + PartialEq,
{
    fn as_mut(&mut self) -> &mut Event<T> {
        &mut self.on_change
    }
}

impl<T> PartialEq for Observable<T>
where
    T: Clone + Send + PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T> PartialEq<T> for Observable<T>
where
    T: Clone + Send + PartialEq,
{
    fn eq(&self, other: &T) -> bool {
        self.value == *other
    }
}

impl<T> Eq for Observable<T> where T: Clone + Send + PartialEq {}
