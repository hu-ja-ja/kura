fn main() {
    winresource::WindowsResource::new()
        .set_icon("assets/kura.ico")
        .compile()
        .expect("the Kura icon resource should compile");
}
