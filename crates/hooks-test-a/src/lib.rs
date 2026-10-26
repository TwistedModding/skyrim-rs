//! The hooks suite's plugin a (`hooks_test::a`).

framework::plugin! {
    name: "skyrim-rs-hooks-a",
    load: hooks_test::a::load,
}
