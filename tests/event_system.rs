#[cfg(test)]
mod tests {

    use std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicU8, Ordering},
        },
    };

    use lum_event::Event;
    use lum_libs::tokio;

    static TEST_EVENT_NAME: &str = "test_event";
    static TEST_CHANNEL_NAME: &str = "test_channel";
    static TEST_ASYNC_CLOSURE_NAME: &str = "test_async_closure";
    static TEST_CLOSURE_NAME: &str = "test_closure";
    static TEST_DATA: &str = "test_data";
    static TEST_ERROR: &str = "test_error";

    #[tokio::test]
    async fn event_new() {
        let event = Event::<String>::new(TEST_EVENT_NAME);

        assert_eq!(event.name, TEST_EVENT_NAME);
        assert_eq!(event.subscriber_count().await, 0);
    }

    #[tokio::test]
    async fn event_subscribe_channel() {
        let event = Event::new(TEST_EVENT_NAME);
        let (_, mut receiver) = event
            .subscribe_channel(TEST_CHANNEL_NAME, 10, false, false)
            .await;

        event.dispatch(TEST_DATA.to_string()).await.unwrap();
        let result = receiver.recv().await.unwrap();

        assert_eq!(event.subscriber_count().await, 1);
        assert_eq!(result, TEST_DATA.to_string());
    }

    #[tokio::test]
    async fn event_subscribe_async_closure() {
        let event = Event::new(TEST_EVENT_NAME);

        event
            .subscribe_async_closure(
                TEST_ASYNC_CLOSURE_NAME,
                move |data| {
                    Box::pin(async move {
                        assert_eq!(data, TEST_DATA.to_string());
                        Ok(())
                    })
                },
                false,
                false,
            )
            .await;

        event.dispatch(TEST_DATA.to_string()).await.unwrap();

        assert_eq!(event.subscriber_count().await, 1);
    }

    #[tokio::test]
    async fn event_subscribe_closure() {
        let event = Event::new(TEST_EVENT_NAME);

        event
            .subscribe_closure(
                TEST_CLOSURE_NAME,
                move |data| {
                    assert_eq!(data, TEST_DATA.to_string());
                    Ok(())
                },
                false,
                false,
            )
            .await;

        event.dispatch(TEST_DATA.to_string()).await.unwrap();

        assert_eq!(event.subscriber_count().await, 1);
    }

    #[tokio::test]
    async fn event_unsubscribe() {
        let event = Event::new(TEST_EVENT_NAME);
        let count = Arc::new(AtomicU8::new(0));

        let count_clone = count.clone();
        let uuid = event
            .subscribe_closure(
                TEST_CLOSURE_NAME,
                move |_data| {
                    count_clone.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
                false,
                false,
            )
            .await;

        assert_eq!(event.subscriber_count().await, 1);
        assert_eq!(count.load(Ordering::Relaxed), 0);

        event.dispatch(TEST_DATA.to_string()).await.unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 1);

        event.dispatch(TEST_DATA.to_string()).await.unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 2);

        let remove_result = event.unsubscribe(&uuid).await;
        assert!(remove_result);
        assert_eq!(event.subscriber_count().await, 0);

        event.dispatch(TEST_DATA.to_string()).await.unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn event_dispatch_with_error() {
        let event = Event::new(TEST_EVENT_NAME);
        event
            .subscribe_closure(
                TEST_CLOSURE_NAME,
                |_data| Err(Box::new(io::Error::other(TEST_ERROR))),
                true,
                true,
            )
            .await;
        assert_eq!(event.subscriber_count().await, 1);

        let result = event.dispatch(TEST_DATA.to_string()).await;
        assert!(result.is_err());
        assert_eq!(event.subscriber_count().await, 0);
    }

    #[tokio::test]
    async fn event_partial_eq() {
        let event1 = Event::<String>::new(format!("{}-{}", TEST_EVENT_NAME, 1));
        let event2 = Event::<String>::new(format!("{}-{}", TEST_EVENT_NAME, 2));
        assert_ne!(event1, event2);
        assert_eq!(event1, event1);
        assert_eq!(event2, event2);
        assert_eq!(event1, event1.uuid);
        assert_eq!(event2, event2.uuid);
    }

    #[test]
    fn event_debug() {
        let event = Event::<String>::new(TEST_EVENT_NAME);
        let debug_str = format!("{event:?}");
        assert!(debug_str.contains("Event"));
        assert!(debug_str.contains("uuid"));
        assert!(debug_str.contains("name"));
        assert!(debug_str.contains("subscribers"));
    }

    #[tokio::test]
    async fn test_display() {
        let event = Event::<String>::new(TEST_EVENT_NAME);
        let display_str = format!("{event}");
        assert_eq!(display_str, "Event test_event");
    }
}
