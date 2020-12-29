use crate::packet::{Error, Packet, PacketId, decode_payload, encode_payload};
use crypto::{digest::Digest, sha1::Sha1};
use rand::{thread_rng, Rng};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::sync::{atomic::AtomicBool, Arc};

enum TransportType {
    Polling(Client),
}

// do we might need a lock here? -> I would say yes, at least for message events
type Callback<I> = Arc<Option<Box<dyn Fn(I)>>>;

struct TransportClient {
    transport: TransportType,
    on_error: Callback<String>,
    on_open: Callback<()>,
    on_close: Callback<()>,
    on_data: Callback<Vec<u8>>,
    on_packet: Callback<Packet>,
    connected: Arc<AtomicBool>,
    address: Option<String>,
    connection_data: Option<HandshakeData>,
}

#[derive(Serialize, Deserialize, Debug)]
struct HandshakeData {
    sid: String,
    upgrades: Vec<String>,
    #[serde(rename = "pingInterval")]
    ping_interval: i32,
    #[serde(rename = "pingTimeout")]
    ping_timeout: i32,
}

impl TransportClient {
    pub fn new() -> Self {
        TransportClient {
            transport: TransportType::Polling(Client::new()),
            on_error: Arc::new(None),
            on_open: Arc::new(None),
            on_close: Arc::new(None),
            on_data: Arc::new(None),
            on_packet: Arc::new(None),
            connected: Arc::new(AtomicBool::default()),
            address: None,
            connection_data: None,
        }
    }

    pub async fn open(&mut self, address: String) -> Result<(), Error> {
        // TODO: Check if Relaxed is appropiate -> change all occurences if not
        if self.connected.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }

        match &mut self.transport {
            TransportType::Polling(client) => {
                // build the query path, random_t is used to prevent browser caching
                let query_path = &format!(
                    "/engine.io/?EIO=4&transport=polling&t={}",
                    TransportClient::get_random_t()
                )[..];

                if let Ok(full_address) = Url::parse(&(address.clone() + query_path)[..]) {
                    self.address = Some(address);

                    let response = dbg!(client.get(full_address).send().await?.text().await?);

                    if let Ok(connection_data) = serde_json::from_str(&response[1..]) {
                        self.connection_data = dbg!(connection_data);

                        if let Some(function) = self.on_open.as_ref() {
                            function(());
                        }
                        return Ok(());
                    }
                    return Err(Error::HandshakeError(response));
                }
                return Err(Error::InvalidUrl(address));
            }
        }
    }

    pub async fn emit(&mut self, packet: Packet) -> Result<(), Error> {
        if self.connected.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::ActionBeforeOpen);
        }
        match &mut self.transport {
            TransportType::Polling(client) => {
                let query_path = &format!(
                    "/engine.io/?EIO=4&transport=polling&t={}&sid={}",
                    TransportClient::get_random_t(),
                    self.connection_data.as_ref().unwrap().sid
                );

                let address =
                    Url::parse(&(self.address.as_ref().unwrap().to_owned() + query_path)[..])
                        .unwrap();

                let data = encode_payload(vec![packet]);
                let status = client
                    .post(address)
                    .body(data)
                    .send()
                    .await?
                    .status()
                    .as_u16();
                if status != 200 {
                    return Err(Error::HttpError(status));
                }

                Ok(())
            }
        }
    }

    async fn poll(&mut self) -> Result<(), Error> {
        if self.connected.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::ActionBeforeOpen);
        }

        match &mut self.transport {
            TransportType::Polling(client) => {
                let query_path = &format!(
                    "/engine.io/?EIO=4&transport=polling&t={}&sid={}",
                    TransportClient::get_random_t(),
                    self.connection_data.as_ref().unwrap().sid
                );

                let address = Url::parse(&(self.address.as_ref().unwrap().to_owned() + query_path)[..])
                .unwrap();

                // TODO: check if to_vec is inefficient here
                let response = client.get(address).send().await?.bytes().await?.to_vec();
                let packets = decode_payload(response)?;
                for packet in packets {
                    // call the packet callback
                    if let Some(function) = self.on_packet.as_ref() {
                        function(packet.clone());
                    }

                    // check for the appropiate action or callback
                    match packet.packet_id {
                        PacketId::Message => {
                            if let Some(function) = self.on_data.as_ref() {
                                function(packet.data);
                            }
                        }
                        PacketId::Close => {
                            dbg!("Received close!");
                            todo!("Close the connection");
                        }
                        PacketId::Open => {
                            dbg!("Received open!");
                            todo!("Think about this, we just receive it in the 'open' method.")
                        }
                        PacketId::Upgrade => {
                            dbg!("Received upgrade!");
                            todo!("Upgrade the connection, but only if possible");
                        }
                        PacketId::Ping => {
                            dbg!("Received ping!");
                            todo!("Update ping state and send pong");
                        }
                        PacketId::Pong => {
                            dbg!("Received pong!");
                            todo!("Won't really happen, just the server sends those");
                        }
                        PacketId::Noop => ()
                    }
                }

                Ok(())
            }
        }
    }

    // Produces a random String that is used to prevent browser caching.
    // TODO: Check if there is a more efficient way
    fn get_random_t() -> String {
        let mut hasher = Sha1::new();
        let mut rng = thread_rng();
        let arr: [u8; 32] = rng.gen();
        hasher.input(&arr);
        hasher.result_str()
    }
}

#[cfg(test)]
mod test {
    use crate::packet::PacketId;
    use std::str;

    use super::*;

    #[actix_rt::test]
    async fn test_connection() {
        let mut socket = TransportClient::new();
        socket
            .open("http://localhost:4200".to_owned())
            .await
            .unwrap();

        socket
            .emit(Packet::new(
                PacketId::Message,
                "HelloWorld".to_string().into_bytes(),
            ))
            .await
            .unwrap();

        socket.on_data = Arc::new(Some(Box::new(|data| {
            println!("Received: {:?}", str::from_utf8(&data).unwrap());
        })));
 
        socket.poll().await.unwrap();
    }
}
