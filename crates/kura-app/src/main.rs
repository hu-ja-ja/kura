use gpui_kit::assets::Assets;
use gpui_kit::component::{
    button::*,
    input::{Input, InputState},
    *,
};
use gpui_kit::*;

struct Root {
    search: Entity<InputState>,
}

impl Root {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search vault"));
        Self { search }
    }
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_4()
            .gap_3()
            .child(div().text_xl().child("Kura"))
            .child(Input::new(&self.search).cleanable(true))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .p_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Vault is locked"),
            )
            .child(
                h_flex().gap_2().child(
                    Button::new("unlock")
                        .primary()
                        .label("Unlock")
                        .on_click(|_, _, _| {}),
                ),
            )
    }
}

fn main() {
    let app = gpui_kit::application().with_assets(Assets);

    app.run(move |cx| {
        gpui_kit::init(cx);

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(900.), px(700.)), cx)),
            ..Default::default()
        };

        gpui_kit::open_window(window_options, cx, |window, cx| {
            cx.new(|cx| Root::new(window, cx))
        })
        .expect("failed to open window");
    });
}
