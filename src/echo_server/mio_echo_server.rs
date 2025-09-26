use mio::event::Event;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};
use std::collections::HashMap;
use std::io;
use std::io::{ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::time::Duration;

struct Connection {
    socket: TcpStream,
    read_buffer: Vec<u8>,
    write_buffer: Vec<u8>,
}

impl Connection {
    fn new(socket: TcpStream) -> Self {
        Connection {
            socket,
            read_buffer: Vec::with_capacity(1024),
            write_buffer: Vec::with_capacity(1024),
        }
    }

    fn read_data(&mut self) -> io::Result<bool> {
        let mut buffer = [0; 1024];
        match self.socket.read(&mut buffer) {
            Ok(0) => Ok(false),
            Ok(n) => {
                self.read_buffer.extend_from_slice(&buffer[..n]);
                self.move_line_to_write_buffer()?;

                Ok(true)
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => Ok(true),
            Err(e) => Err(e),
        }
    }

    fn write_data(&mut self) -> io::Result<bool> {
        match self.socket.write(&mut self.write_buffer) {
            Ok(n) => {
                self.write_buffer.drain(..n);
                Ok(true)
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => Ok(true),
            Err(e) => Err(e),
        }
    }

    fn move_line_to_write_buffer(&mut self) -> io::Result<()> {
        let linebreak_byte = b'\n';
        if let Some(linebreak_pos) = self
            .read_buffer
            .iter()
            .position(|&byte| byte == linebreak_byte)
        {
            let line = self
                .read_buffer
                .drain(..linebreak_pos + 1)
                .collect::<Vec<u8>>();
            self.write_buffer.extend_from_slice(&line);
        }

        Ok(())
    }

    fn is_writable(&self) -> bool {
        !self.write_buffer.is_empty()
    }
}

const LISTENER_TOKEN: Token = Token(0);

struct MioEchoServer {
    listener: TcpListener,
    poll: Poll,
    connections: HashMap<Token, Connection>,
    last_token_id: usize,
}

impl MioEchoServer {
    fn new(addr: SocketAddr) -> io::Result<Self> {
        let mut listener = TcpListener::bind(addr)?;
        let poll = Poll::new()?;
        poll.registry()
            .register(&mut listener, LISTENER_TOKEN, Interest::READABLE)?;

        Ok(Self {
            listener,
            poll,
            connections: HashMap::new(),
            last_token_id: LISTENER_TOKEN.0,
        })
    }

    fn run(&mut self, poll_timeout: Duration) -> io::Result<()> {
        let mut events = Events::with_capacity(1024);

        loop {
            self.poll.poll(&mut events, Some(poll_timeout))?;

            for event in events.iter() {
                match event.token() {
                    LISTENER_TOKEN => self.accept_connection()?,
                    token => self.handle_connection(token, &event)?,
                }
            }
        }
    }

    fn accept_connection(&mut self) -> io::Result<()> {
        let (mut socket, addr) = self.listener.accept()?;
        println!("Accepted connection from {}", addr);
        let token = self.make_token();
        self.poll
            .registry()
            .register(&mut socket, token, Interest::READABLE)?;

        self.connections.insert(token, Connection::new(socket));
        Ok(())
    }

    fn handle_connection(&mut self, token: Token, event: &Event) -> io::Result<()> {
        let conn = self.connections.get_mut(&token).unwrap();

        if event.is_readable() {
            match conn.read_data() {
                Ok(true) => {
                    if conn.is_writable() {
                        self.poll.registry().reregister(
                            &mut conn.socket,
                            token,
                            Interest::READABLE | Interest::WRITABLE,
                        )?;
                    }
                }
                Ok(false) | Err(_) => {
                    self.remove_connection(token)?;
                    return Ok(());
                }
            }
        }

        if event.is_writable() {
            match conn.write_data() {
                Ok(true) => {
                    if !conn.is_writable() {
                        self.poll.registry().reregister(
                            &mut conn.socket,
                            token,
                            Interest::READABLE,
                        )?;
                    }
                }
                Ok(false) | Err(_) => {
                    self.remove_connection(token)?;
                }
            }
        }

        Ok(())
    }

    fn make_token(&mut self) -> Token {
        self.last_token_id += 1;
        Token(self.last_token_id)
    }

    fn remove_connection(&mut self, token: Token) -> io::Result<()> {
        if let Some(mut conn) = self.connections.remove(&token) {
            self.poll.registry().deregister(&mut conn.socket)?;
        }

        Ok(())
    }
}

pub fn run() {
    let addr = "127.0.0.1:10000".parse().unwrap();
    let mut server = MioEchoServer::new(addr).unwrap();
    let poll_timeout = Duration::from_millis(1000);
    server.run(poll_timeout).unwrap();
}
