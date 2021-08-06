pub struct Template {
    cont: String,
}

impl Template {
    pub fn new(cont: String) -> Self {
        Template { cont }
    }

    pub fn assign(&mut self, name: &str, val: &str) -> &mut Self {
        self.cont = self.cont.replace(&format!("{{{{{}}}}}", name), val);
        self
    }

    pub fn render(&self) -> String {
        self.cont.clone()
    }
}
