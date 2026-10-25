pub(super) fn markdown(text: &str) -> String {
    format!(
        "---\ntitle: Writer\ntype: component\ntags: [rust]\naliases: [WriterLoop]\n---\n{text}\n"
    )
}
