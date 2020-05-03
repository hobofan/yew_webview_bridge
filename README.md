## yew_webview_bridge - 2-way communcation between yew and web-view

This crate provides a 2-way communcation bridge between [web-view](https://github.com/Boscop/web-view) and [yew](https://github.com/yewstack/yew).

For the frontend, it provides a yew service (`WebViewMessageService`), with which components can easily send (and receive matching responses) to web-view.

For the backend, it provides an easy handler that can be used to respond to messages from the frontend.

Internally, unique random IDs are generated for every service and message, so that responses can be matched up with the correct component and message.

## Installation

The crate has to be included in both the frontend crate (the one with yew) and the backend/host crate (the one with web-view), with the appropriate feature flags.

### Frontend crate

```
[dependencies]
yew_webview_bridge = { version = "0.1.0", features = ["frontend"] }
```

### Backend crate

```
[dependencies]
yew_webview_bridge = { version = "0.1.0", features = ["backend"] }
```

## Usage

### Frontend (yew)

```rust
use yew_webview_bridge::frontend::*;

pub struct MyComponent {
  webview: WebViewMessageService,
  // .. other fields
}

// In one of the components methods (e.g. update)
send_future(
    &self.link,
    self.webview
        .send_message(self.state.value.clone()) // The value you want to send
        .map(|res: String| Msg::AddStr(res)), // Mapping the result to a component Msg
);
```

### Backend (web-view)

```rust
use yew_webview_bridge::backend::*;

web_view::builder()
    // all the other options
    .invoke_handler(|webview, arg| {
        handle_yew_message(webview, arg, |message: String| {
            // If we return a Some(_), we send a response for this message
            Some(format!("Hello, {}", &message))
        });
        Ok(())
    })
    .run()
    .unwrap();
```

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
