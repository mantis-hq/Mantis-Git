use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::*;

struct Root;

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .size_full()
            .items_center()
            .justify_center()
            .child("gitclient")
            .child(
                Button::new("ok")
                    .primary()
                    .label("Hello")
                    .on_click(|_, _, _| println!("Clicked!")),
            )
    }
}

fn main() {
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Root))
            .expect("Failed to open window");
    });
}
