use serde::{Deserialize, Serialize};

/// The message passed between the backend and frontend. Includes
/// associated metadata ensuring that the message is delivered to the
/// intended `WebViewMessageService` and `MessageFuture` waiting for a
/// reply.
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

    /// Generate a new message id.
    pub fn generate_message_id() -> u32 {
        rand::thread_rng().gen()
    }

    /// Create a message for the provided subscription id.
    pub fn for_subscription_id(subscription_id: u32, inner: T) -> Self {
        Self {
            subscription_id,
            message_id: Self::generate_message_id(),
            inner,
        }
    }
}

use rand::Rng;
#[cfg(feature = "frontend")]
use wasm_bindgen::prelude::wasm_bindgen;
use std::task::Waker;

#[cfg(feature = "frontend")]
#[wasm_bindgen(module = "/src/js/invoke_webview.js")]
extern "C" {
    fn invoke_webview(message: String);
}

/// The waker and the message data for a given message id. When these
/// are set to `Some`, the `Future` waiting for the message will poll
/// `Ready`.
struct WakerMessage {
    pub waker: Option<Waker>,
    pub message: Option<serde_json::Value>,
}

impl WakerMessage {
    pub fn new() -> Self {
        WakerMessage {
            waker: None,
            message: None,
        }
    }
}

impl Default for WakerMessage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "frontend")]
pub mod frontend {
    //! This module should be enabled (using the `frontend` feature)
    //! for use in a Rust front-end using the `yew` framework compiled
    //! to WASM, and running in `web-view`.
    
    pub use super::Message;
    use crate::{WakerMessage, invoke_webview};
    use dashmap::DashMap;
    use serde::{de::DeserializeOwned, Deserialize, Serialize};
    use std::future::Future;
    use std::marker::PhantomData;
    use std::pin::Pin;
    use std::sync::{Arc, RwLock};
    use std::task::{Context, Poll};
    use wasm_bindgen::{prelude::Closure, JsCast, JsValue};
    use web_sys::{Document, Window, EventListener, CustomEvent};
    use yew::prelude::{Component, ComponentLink};

    /// A map of message ids
    /// ([Message#message_id](Message#message_id)), to the `Waker` for
    /// the task which is waiting, and the message data. When the
    /// [WakerMessage](WakerMessage) data is set, the `Future` waiting
    /// for the message will poll `Ready`.
    type MessageFuturesMap =
        Arc<DashMap<u32, Arc<RwLock<WakerMessage>>>>;

    static LISTENER_TYPE: &'static str = "yew-webview-bridge-response";

    /// This is a service that can be used within a `yew` `Component`
    /// to communicate with the backend which is hosting the
    /// `web-view` that the frontend is running in. 
    ///
    /// To listen to messages from the backend, this service attaches
    /// a listener to the `document` (`document.addListener(...)`).
    pub struct WebViewMessageService {
        /// A unique identifier for the messages communicating with this service.
        subscription_id: u32,
        message_futures_map: MessageFuturesMap,
        event_listener: EventListener,
        _event_listener_closure: Closure<dyn Fn(CustomEvent)>,
    }

    impl Default for WebViewMessageService {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for WebViewMessageService {
        /// Removes the event listener.
        fn drop(&mut self) {
            let window: Window = web_sys::window().unwrap();
            let document: Document = window.document().unwrap();
            document
                .remove_event_listener_with_event_listener(
                    LISTENER_TYPE,
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
                    Self::response_handler(subscription_id, listener_futures_map.clone(), event);
                }));

            let mut listener = EventListener::new();
            listener.handle_event(closure.as_ref().unchecked_ref());

            document
                .add_event_listener_with_event_listener(
                    LISTENER_TYPE,
                    &listener,
                )
                .expect("unable to register yew-webview-bridge-response callback");

            Self {
                subscription_id,
                message_futures_map,
                event_listener: listener,
                _event_listener_closure: closure,
            }
        }

        /// Handle an event coming from the `web-view` backend, and
        /// respond to it, resolving/waking the relevent pending
        /// future (with matching message id), if there are any.
        fn response_handler(
            subscription_id: u32,
            message_futures_map: MessageFuturesMap,
            event: CustomEvent,
        ) {
            let detail: JsValue = event.detail();
            let message_str: String = detail.as_string().expect("expected event detail to be a String");
            let message: Message<serde_json::Value> = serde_json::from_str(&message_str).unwrap();

            if message.subscription_id != subscription_id {
                return;
            }
            if !message_futures_map.contains_key(&message.message_id) {
                return;
            }

            let future_value = message_futures_map.remove(&message.message_id).unwrap().1;

            let mut future_value_write = future_value.write().unwrap();
            future_value_write.message = Some(message.inner);
            future_value_write.waker.as_ref().map(|waker| {
                waker.wake_by_ref();
            });
        }

        fn new_result_future<T>(&self, message_id: u32) -> MessageFuture<T> {
            let future = MessageFuture::new();

            self.message_futures_map
                .insert(message_id, future.message.clone());

            future
        }

        /// Serialize a message and send it to the `web-view` backend.
        fn send_message_to_webview<T: Serialize>(message: T) {
            let message_serialized =
                serde_json::to_string(&message).expect("unable to serialize message");

            #[allow(unused_unsafe)]
            unsafe {
                invoke_webview(message_serialized);
            }
        }

        /// Create a new message for this service instance's subscription.
        fn new_message<'a, T: Serialize + Deserialize<'a>>(&self, message: T) -> Message<T> {
            Message::for_subscription_id(self.subscription_id, message)
        }

        /// Send a message to the `web-view` backend, receive a future
        /// for a reply message.
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

    /// A message that will be received in the future.
    pub struct MessageFuture<T> {
        message: Arc<RwLock<WakerMessage>>,
        return_type: PhantomData<T>,
    }

    impl<T> MessageFuture<T> {
        pub fn new() -> Self {
            Self {
                message: Arc::new(RwLock::new(WakerMessage::default())),
                return_type: PhantomData,
            }
        }
    }

    impl<T: DeserializeOwned> Future for MessageFuture<T> {
        type Output = T;

        fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
            let mut waker_message = self.message.write().expect("unable to obtain RwLock on message");

            if (*waker_message).message.is_none() {
                waker_message.waker = Some(cx.waker().clone());
                return Poll::Pending;
            }

            let message = waker_message.message.as_ref().unwrap();
            let result = serde_json::from_value(message.clone());
            let value = result.expect("unable to deserialize message");
            Poll::Ready(value)
        }
    }

    /// Send a future which is expected to recieve a reply message when
    /// it is ready, and forward the reply message to the specified
    /// component when it is received.
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

        #[allow(unused_must_use)]
        {
            future_to_promise(js_future);
        }
    }
}

#[cfg(feature = "backend")]
pub mod backend {
    //! This module should be enable (using the `backend` feature) for
    //! use in the the backend of the application which is hosting the
    //! `web-view` window.
    
    pub use super::Message;
    use serde::{Deserialize, Serialize};
    use web_view::WebView;

    /// Handle a `web-view` message as a message coming from `yew`.
    /// 
    /// # Example
    /// 
    /// ```no_run
    /// use yew_webview_bridge::backend::handle_yew_message;
    /// use web_view;
    /// 
    /// web_view::builder()
    ///     .content(web_view::Content::Html("<html></html>"))
    ///     .user_data(())
    ///     .invoke_handler(|webview, arg| {
    ///         handle_yew_message(webview, arg, |message: String| {
    ///             // If we return a Some(_), we send a response for this message
    ///             Some("reply".to_string())
    ///         });
    ///         Ok(())
    ///     });
    /// ```
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

    /// Send a response a [Message](Message) recieved from the frontend via
    /// `web-view`'s `eval()` method.
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
