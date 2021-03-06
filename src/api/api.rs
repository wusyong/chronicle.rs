use tokio::sync::mpsc;

app!(ApiBuilder {
    listen_address: String
});

impl ApiBuilder {
    pub fn build(self) -> Api {
        Api {
            listen_address: self.listen_address.unwrap(),
            launcher_tx: self.launcher_tx,
        }
    }
}

pub struct Api {
    listen_address: String,
    launcher_tx: Option<mpsc::UnboundedSender<String>>,
}

impl Api {
    pub async fn run(mut self) {
        unimplemented!()
    }
}
