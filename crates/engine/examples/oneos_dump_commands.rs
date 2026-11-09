//! Prints every registry command (id, menu, label, params), tab separated. A OneOS survey tool.
fn main() {
    for c in filmcraft_engine::command_specs() {
        println!("{}\t{}\t{}\t{}", c.id, c.menu.join("/"), c.label, c.params.replace(['\n', '\t'], " "));
    }
}
