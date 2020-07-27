use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
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

use rand::Rng;
#[cfg(feature = "frontend")]
use wasm_bindgen::prelude::wasm_bindgen;

#[cfg(feature = "frontend")]
#[wasm_bindgen(module = "/src/js/invoke_webview.js")]
extern "C" {
    fn invoke_webview(message: String);
}

#[cfg(feature = "frontend")]
pub mod frontend {
    pub use super::Message;
    use crate::invoke_webview;
    use dashmap::DashMap;
    use serde::{de::DeserializeOwned, Deserialize, Serialize};
    use std::future::Future;
    use std::marker::PhantomData;
    use std::pin::Pin;
    use std::sync::{Arc, RwLock};
    use std::task::{Context, Poll, Waker};
    use wasm_bindgen::{prelude::Closure, JsCast, JsValue};
    use web_sys::{Document, Window, EventListener, console, CustomEvent};
    use js_sys::Function;
    use yew::prelude::{Component, ComponentLink};

    type MessageFuturesMap =
        Arc<DashMap<u32, Arc<RwLock<(Option<Waker>, Option<serde_json::Value>)>>>>;

    pub struct WebViewMessageService {
        subscription_id: u32,
        message_futures_map: MessageFuturesMap,
        event_listener: EventListener,
        event_listener_closure: Closure<dyn Fn(CustomEvent)>,
        event_listener_function: Function,
    }

    impl Default for WebViewMessageService {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for WebViewMessageService {
        fn drop(&mut self) {
            let window: Window = web_sys::window().unwrap();
            let document: Document = window.document().unwrap();
            document
                .remove_event_listener_with_event_listener(
                    "yew-webview-bridge-response",
                    &self.event_listener,
                )
                .expect("unable to remove yew-webview-bridge-response listener");
        }
    }

    impl WebViewMessageService {
        pub fn new() -> Self {
            let subscription_id = Message::<()>::generate_subscription_id();
            let message_futures_map = Arc::new(DashMap::new());

            let window: Window = web_sys::window().unwrap();
            let document: Document = window.document().unwrap();

            let listener_futures_map = message_futures_map.clone();
            let closure: Closure<dyn Fn(CustomEvent)> =
                Closure::wrap(Box::new(move |event: CustomEvent| {
                    log::debug!("Event detail: {:?}", event.detail());
                    Self::response_handler(subscription_id, listener_futures_map.clone(), event);
                }));

            let function = Function::new_with_args("event", "console.log(\"js inline responding to event: \", event);");

            let mut listener = EventListener::new();
            listener.handle_event(closure.as_ref().unchecked_ref());

            document
                .add_event_listener_with_event_listener(
                    "yew-webview-bridge-response",
                    &listener,
                )
                .expect("unable to register yew-webview-bridge-response callback");

            Self {
                subscription_id,
                message_futures_map,
                event_listener: listener,
                event_listener_closure: closure,
                event_listener_function: function,
            }
        }

        fn response_handler(
            subscription_id: u32,
            message_futures_map: MessageFuturesMap,
            event: CustomEvent,
        ) {
            let detail: JsValue = event.detail();
            log::debug!("Detail: {:?}", detail);
            let message_str: String = detail.as_string().expect("expected event detail to be a String");
            log::debug!("Message string: {}", message_str);
            let message: Message<serde_json::Value> = serde_json::from_str(&message_str).unwrap();

            if message.subscription_id != subscription_id {
                return;
            }
            if !message_futures_map.contains_key(&message.message_id) {
                return;
            }

            let future_value = message_futures_map.remove(&message.message_id).unwrap().1;

            let mut future_value_write = future_value.write().unwrap();
            future_value_write.1 = Some(message.inner);
            future_value_write.0.as_ref().map(|waker| {
                log::debug!("Mapping waker: {:?}", waker);
                waker.wake_by_ref();
            });
        }

        fn new_result_future<T>(&self, message_id: u32) -> MessageFuture<T> {
            let future = MessageFuture::new();

            self.message_futures_map
                .insert(message_id, future.message.clone());

            future
        }

        fn send_message_to_webview<T: Serialize>(message: T) {
            let message_serialized =
                serde_json::to_string(&message).expect("unable to serialize message");
            unsafe { invoke_webview(message_serialized) };
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

            log::debug!("successfully obtained message write lock");

            if (*message).1.is_none() {
                message.0 = Some(cx.waker().clone());
                return Poll::Pending;
            }

            log::debug!("message is not none: {:?}", message.1);

            let message = message.1.as_ref().unwrap();

            log::debug!("Successfully obtained message as ref: {:?}", message);

            let result = serde_json::from_value(message.clone());

            if let Err(error) = &result {
                log::debug!("Message deserialization error: {}", error);
            }
            

            let value = result.expect("unable to deserialize message");
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
        let message_string = serde_json::to_string(&message).unwrap();
        println!("Message string: {}", message_string);
        let eval_script = format!(
            r#"
        document.dispatchEvent(
            new CustomEvent("{event_name}", {{ detail: {message:?} }})
        );"#,
            event_name = "yew-webview-bridge-response",
            message = message_string
        );
        webview
            .eval(&eval_script)
            .expect("failed to dispatch event to yew");
    }
}
