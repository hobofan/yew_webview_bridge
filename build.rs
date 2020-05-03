pub fn main() {
    let using_frontend = cfg!(feature = "frontend");
    let using_backend = cfg!(feature = "backend");
    if !using_frontend && !using_backend {
        panic!("yew-webview-bridge requires selecting either the `frontend` or `backend` cargo feature");
    }
}
