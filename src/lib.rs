use rand::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct Message<T> {
    pub subscription_id: u32,
    pub message_id: u32,
    pub inner: T,
}

impl<'a, T: Deserialize<'a> + Serialize> Message<T> {
    pub fn generate_subscription_id() -> u32 {
        rand::thread_rng().gen()
    }

    pub fn generate_message_id() -> u32 {
        rand::thread_rng().gen()
    }

    pub fn for_subscription_id(subscription_id: u32, inner: T) -> Self {
        Self {
            subscription_id,
            message_id: Self::generate_subscription_id(),
            inner,
        }
    }
}

#[cfg(feature = "frontend")]
pub mod frontend {
    pub use super::Message;
    use dashmap::DashMap;
    use serde::{de::DeserializeOwned, Deserialize, Serialize};
    use std::future::Future;
    use std::marker::PhantomData;
    use std::pin::Pin;
    use std::sync::{Arc, RwLock};
    use std::task::{Context, Poll, Waker};
    use stdweb::{js, unstable::TryInto, Value};
    use wasm_bindgen::JsValue;
    use yew::prelude::{Component, ComponentLink};

    type MessageFuturesMap =
        Arc<DashMap<u32, Arc<RwLock<(Option<Waker>, Option<serde_json::Value>)>>>>;

    pub struct WebViewMessageService {
        subscription_id: u32,
        message_futures_map: MessageFuturesMap,
        event_listener: Value,
    }

    impl Default for WebViewMessageService {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for WebViewMessageService {
        fn drop(&mut self) {
            js! {
                var value = @{&self.event_listener};
                document.removeEventListener(value.name, value.listener);
                value.callback.drop();
            }
        }
    }

    impl WebViewMessageService {
        pub fn new() -> Self {
            let subscription_id = Message::<()>::generate_subscription_id();
            let message_futures_map = Arc::new(DashMap::new());

            let callback = Self::response_handler(subscription_id, message_futures_map.clone());

            let js_callback = js! {
                var callback = @{callback};
                var listener = event => callback(event.detail);
                document.addEventListener("yew-webview-bridge-response", listener);
                return {
                    name: "yew-webview-bridge-response",
                    callback: callback,
                    listener: listener
                };
            };

            Self {
                subscription_id,
                message_futures_map,
                event_listener: js_callback,
            }
        }

        fn response_handler(
            subscription_id: u32,
            message_futures_map: MessageFuturesMap,
        ) -> Box<dyn Fn(Value)> {
            Box::new(move |value: Value| {
                let message_str: String = value
                    .try_into()
                    .expect("unable to parse payload from event");
                let message: Message<serde_json::Value> =
                    serde_json::from_str(&message_str).unwrap();

                if message.subscription_id != subscription_id {
                    return;
                }
                if !message_futures_map.contains_key(&message.message_id) {
                    return;
                }

                let future_value = message_futures_map.remove(&message.message_id).unwrap().1;

                future_value.write().unwrap().1 = Some(message.inner);
                future_value
                    .write()
                    .unwrap()
                    .0
                    .as_ref()
                    .map(|n| n.wake_by_ref());
            })
        }

        fn new_result_future<T>(&self, message_id: u32) -> MessageFuture<T> {
            let future = MessageFuture::new();

            self.message_futures_map
                .insert(message_id, future.message.clone());

            future
        }

        fn send_message_to_webview<T: Serialize>(message: T) {
            js! {
                window.external.invoke(@{serde_json::to_string(&message).unwrap()});
            }
        }

        fn new_message<'a, T: Serialize + Deserialize<'a>>(&self, message: T) -> Message<T> {
            Message::for_subscription_id(self.subscription_id, message)
        }

        pub fn send_message<
            'a,
            IN: Serialize + Deserialize<'a>,
            OUT: Serialize + Deserialize<'a>,
        >(
            &self,
            message: IN,
        ) -> MessageFuture<OUT> {
            let message = self.new_message(message);
            let message_res = self.new_result_future(message.message_id);

            Self::send_message_to_webview(message);

            message_res
        }
    }

    pub struct MessageFuture<T> {
        message: Arc<RwLock<(Option<Waker>, Option<serde_json::Value>)>>,
        return_type: PhantomData<T>,
    }

    impl<T> MessageFuture<T> {
        pub fn new() -> Self {
            Self {
                message: Arc::new(RwLock::new((None, None))),
                return_type: PhantomData,
            }
        }
    }

    impl<T: DeserializeOwned> Future for MessageFuture<T> {
        type Output = T;

        fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let mut message = self.message.write().unwrap();
            if (*message).1.is_none() {
                message.0 = Some(cx.waker().clone());
                return Poll::Pending;
            }

            let message = message.1.as_ref().unwrap();
            let value = serde_json::from_value(message.clone()).unwrap();
            Poll::Ready(value)
        }
    }

    #[allow(unused_must_use)]
    pub fn send_future<COMP: Component, F>(link: &ComponentLink<COMP>, future: F)
    where
        F: Future<Output = COMP::Message> + 'static,
    {
        use wasm_bindgen_futures::future_to_promise;

        let link = link.clone();
        let js_future = async move {
            link.send_message(future.await);
            Ok(JsValue::NULL)
        };

        future_to_promise(js_future);
    }
}

#[cfg(feature = "backend")]
pub mod backend {
    pub use super::Message;
    use serde::{Deserialize, Serialize};
    use web_view::WebView;

    pub fn handle_yew_message<
        'a,
        T,
        IN: Deserialize<'a> + Serialize,
        OUT: Deserialize<'a> + Serialize,
        H: Fn(IN) -> Option<OUT>,
    >(
        webview: &mut WebView<T>,
        arg: &'a str,
        handler: H,
    ) {
        let in_message: Message<IN> = serde_json::from_str(&arg).unwrap();

        let output = handler(in_message.inner);
        if let Some(response) = output {
            let out_message = Message {
                subscription_id: in_message.subscription_id,
                message_id: in_message.message_id,
                inner: response,
            };

            send_response_to_yew(webview, out_message);
        }
    }

    fn send_response_to_yew<T, M: Serialize>(webview: &mut WebView<T>, message: Message<M>) {
        let eval_script = format!(
            r#"document.dispatchEvent(
            new CustomEvent("{event_name}", {{ detail: {message:?} }})
        );"#,
            event_name = "yew-webview-bridge-response",
            message = serde_json::to_string(&message).unwrap()
        );
        webview
            .eval(&eval_script)
            .expect("failed to dispatch event to yew");
    }
}
