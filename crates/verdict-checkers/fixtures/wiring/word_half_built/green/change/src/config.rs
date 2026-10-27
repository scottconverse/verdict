pub struct Config {
    pub name: String,
}

pub fn parse_config(text: &str) -> Config {
    Config {
        name: text.trim().to_string(),
    }
}
