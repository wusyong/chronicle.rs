// stage supervisor
// uses
use crate::engine::cluster::node::stage::reporter::{StreamId, StreamIds};
use crate::engine::cluster::node::supervisor::StageNum;
use crate::engine::cluster::supervisor::Address;
use super::reporter;
use super::sender;
use super::receiver;
use std::char;
use tokio::sync::mpsc;
use std::collections::HashMap;
use tokio::net::TcpStream;
use tokio::time::delay_for;
use std::time::Duration;
use crate::engine::cluster::node;

// types
pub type ReporterNum = u8;
pub type Sender = mpsc::UnboundedSender<Event>;
pub type Receiver = mpsc::UnboundedReceiver<Event>;
pub type Reporters = HashMap<ReporterNum,mpsc::UnboundedSender<reporter::Event>>;

// suerpvisor state struct
struct State {
    session_id: usize,
    reporters: Reporters,
    reconnect_requests: u8,
    tx: Sender,
    rx: Receiver,
    connected: bool,
    shutting_down: bool,
    address: Address,
    shard: StageNum,
    reporters_num: u8,
    supervisor_tx: node::supervisor::Sender,
}

// Arguments struct
pub struct Args {
    pub address: Address,
    pub reporters_num: ReporterNum,
    pub shard: u8,
    pub tx: Sender,
    pub rx: Receiver,
    pub supervisor_tx: node::supervisor::Sender,
}


// event Enum
pub enum Event {
    Connect(sender::Sender, sender::Receiver, bool),
    Reconnect(usize),
    Shutdown,
}


pub async fn supervisor(args: Args) -> () {
    // init supervisor
    let State {mut reporters, shard, address, tx, mut rx, mut session_id, mut reconnect_requests,
         mut connected, mut shutting_down, reporters_num, supervisor_tx} = init(args).await;
    // we create sender's channel in advance.
    let (sender_tx, sender_rx) = mpsc::unbounded_channel::<sender::Event>();
    // preparing range to later create stream_ids vector per reporter
    let (mut start_range, appends_num): (StreamId, i16) = (0,32767/(reporters_num as i16));
    // the following for loop will start reporters
    for reporter_num in 0..reporters_num {
        // we create reporter's channel in advance.
        let (reporter_tx, reporter_rx) = mpsc::unbounded_channel::<reporter::Event>();
        // add reporter to reporters map.
        reporters.insert(reporter_num, reporter_tx.clone());
        // start reporter.
        let last_range = start_range+appends_num;
        let stream_ids: StreamIds = ((if reporter_num == 0 {
            1 // we force first reporter_num to start range from 1, as we reversing stream_id=0 for future uses.
        } else {
            start_range // otherwise we keep the start_range as it's
        })..last_range).collect();
        start_range = last_range;
        let reporter_args = reporter::Args{reporter_num, session_id,
            sender_tx: sender_tx.clone(), supervisor_tx: tx.clone(),
            stream_ids, tx: reporter_tx, rx: reporter_rx, shard: shard.clone(),
            address: address.clone()};
        let reporter = reporter::reporter(reporter_args);
        tokio::spawn(reporter);
    }
    // send self event::connect.
    tx.send(Event::Connect(sender_tx,sender_rx, false)); // false because they already have the sender_tx
    // supervisor event_loop
    while let Some(event) = rx.recv().await {
        match event {
            Event::Connect(sender_tx, sender_rx, reconnect) => {
                if !shutting_down { // we only try to connect if the stage not shutting_down.
                    match TcpStream::connect(address.clone()).await {
                        Ok(stream) => {
                            // change the connected status to true
                            connected = true;
                            session_id += 1; // TODO convert the session_id to a meaningful (timestamp + count)
                            // split the stream
                            let (socket_rx, socket_tx) = tokio::io::split(stream);
                            // spawn/restart sender
                            let sender_args = sender::Args{
                                reconnect: reconnect,
                                tx: sender_tx,
                                rx: sender_rx,
                                session_id: session_id,
                                socket_tx: socket_tx, reporters: reporters.clone(),
                                supervisor_tx: tx.clone(),
                            };
                            tokio::spawn(sender::sender(sender_args));
                            // spawn/restart receiver
                            let receiver_args = receiver::Args{
                                socket_rx: socket_rx, reporters: reporters.clone(),
                                supervisor_tx: tx.clone(), session_id: session_id};
                            tokio::spawn(receiver::receiver(receiver_args));

                            if !reconnect {
                                // TODO now reporters are ready to be exposed to workers.. (ex evmap ring.)
                                // create key which could be address:shard (ex "127.0.0.1:9042:5")
                                let event = node::supervisor::Event::Expose(shard,reporters.clone());
                                supervisor_tx.send(event);
                                println!("just exposed stage reporters of shard: {}, to node supervisor", shard);
                            }

                        },
                        Err(err) => {
                            println!("trying to connect every 5 seconds: err {}", err);
                            delay_for(Duration::from_millis(5000)).await;
                            // try again to connect
                            tx.send(Event::Connect(sender_tx, sender_rx,reconnect));
                        },
                    }

                }
            },
            Event::Reconnect(_) if reconnect_requests != reporters_num-1 => {
                // supervisor requires reconnect_requests from all its reporters in order to reconnect.
                // so in this scope we only count the reconnect_requests up to reporters_num-1, which means only one is left behind.
                reconnect_requests += 1;
            },
            Event::Reconnect(_) => {
                // the last reconnect_request from last reporter,
                reconnect_requests = 0; // reset reconnect_requests to zero
                // change the connected status
                connected = false;
                // create sender's channel
                let (sender_tx, sender_rx) = mpsc::unbounded_channel::<sender::Event>();
                tx.send(Event::Connect(sender_tx, sender_rx,true));
            }
            Event::Shutdown => {
                shutting_down = true;
                if !connected { // this will make sure both sender and receiver of the stage are dead.
                    // therefore now we tell reporters to gracefully shutdown
                    for (_,reporter_tx) in reporters.drain() {
                        reporter_tx.send(reporter::Event::Session(reporter::Session::Shutdown));
                    }
                    // finally close rx channel
                    rx.close();
                } else {
                    // wait for 5 second
                    delay_for(Duration::from_secs(5)).await;
                    // trap self with shutdown event.
                    tx.send(Event::Shutdown);
                }

            }
        }
    }
}

async fn init(args: Args) -> State {
    let tx = args.tx;
    let rx = args.rx;
    let shard = args.shard;
    let address = args.address;
    let reporters_num = args.reporters_num;
    let supervisor_tx = args.supervisor_tx;
    // generate vector with capcity of reporters_num
    let reporters: Reporters = HashMap::with_capacity(reporters_num as usize);
    // return state
    State {supervisor_tx, reporters, tx, rx,shard,address,reporters_num,
        session_id: 0, reconnect_requests: 0, connected: false, shutting_down: false}
}
