wit_bindgen::generate!({
    world: "minimal",
    path: "../../wit/minimal.wit",
});

struct Minimal;

impl Guest for Minimal {
    fn process(count: i64) -> i64 {
        count.wrapping_sub(1)
    }
}

export!(Minimal);
