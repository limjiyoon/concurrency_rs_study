use futures::task::{ArcWake, waker_ref};
use mio::net::{TcpListener as MioTcpListener, TcpStream as MioTcpStream};
use mio::{Events, Interest, Poll as MioPoll, Token, Waker as MioWaker};
use std::collections::HashMap;
use std::future::Future;
use std::io::{self, ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

struct Task {
    future: Mutex<Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>>,
    ready_queue: mpsc::Sender<Arc<Task>>,
    mio_waker: Arc<MioWaker>,
}

impl Task {
    fn new(
        future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        ready_queue: mpsc::Sender<Arc<Task>>,
        mio_waker: Arc<MioWaker>,
    ) -> Self {
        Self {
            future: Mutex::new(Some(future)),
            ready_queue,
            mio_waker,
        }
    }

    fn poll(self: &Arc<Self>) {
        let waker = waker_ref(self);
        let mut cx = Context::from_waker(&*waker);
        let mut slot = self.future.lock().unwrap();
        if let Some(future) = slot.as_mut() {
            if let Poll::Ready(()) = future.as_mut().poll(&mut cx) {
                *slot = None;
            }
        }
    }
}

impl ArcWake for Task {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.ready_queue.send(arc_self.clone()).unwrap();
        arc_self.mio_waker.wake().unwrap();
    }
}

pub struct Executor {
    inner: Arc<Mutex<ExecutorInner>>,
}

struct ExecutorInner {
    poll: MioPoll,
    mio_waker: Arc<MioWaker>,
    wakers: HashMap<Token, Waker>,
    last_token_id: usize,
    ready_queue: Option<mpsc::Sender<Arc<Task>>>,
}

impl Executor {
    fn new() -> io::Result<Self> {
        let poll = MioPoll::new()?;
        let last_token_id = 0;
        let mio_waker = Arc::new(MioWaker::new(poll.registry(), Token(last_token_id))?);
        Ok(Self {
            inner: Arc::new(Mutex::new(ExecutorInner {
                poll,
                mio_waker,
                wakers: HashMap::new(),
                last_token_id, // 0 is reserved
                ready_queue: None,
            })),
        })
    }

    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (ready_tx, mio_waker) = {
            let inner = self.inner.lock().unwrap();
            let ready_tx = inner
                .ready_queue
                .as_ref()
                .expect("Spawn called before run")
                .clone();

            (ready_tx, inner.mio_waker.clone())
        };
        let task = Arc::new(Task::new(Box::pin(future), ready_tx.clone(), mio_waker));
        let _ = ready_tx.send(task);
        let _ = self.inner.lock().unwrap().mio_waker.wake();
    }

    fn run<F, R>(&self, future: F) -> R
    where
        F: Future<Output = R> + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<Arc<Task>>();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.ready_queue = Some(tx.clone());
        }
        self.spawn(async move {
            future.await;
        });

        loop {
            while let Some(task) = rx.try_recv().ok() {
                task.poll();
            }

            if let Err(_) = self.poll_events(Duration::from_millis(10)) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn poll_events(&self, timeout: Duration) -> io::Result<()> {
        let mut events = Events::with_capacity(1024);
        let mut inner = self.inner.lock().unwrap();
        inner.poll.poll(&mut events, Some(timeout))?;
        let wakers_to_wake = events
            .iter()
            .filter_map(|event| inner.wakers.remove(&event.token()))
            .collect::<Vec<_>>();

        for waker in wakers_to_wake {
            waker.wake_by_ref();
        }

        Ok(())
    }

    fn register<S: mio::event::Source>(
        &self,
        source: &mut S,
        interest: Interest,
    ) -> io::Result<Token> {
        let mut inner = self.inner.lock().unwrap();
        inner.last_token_id += 1;
        let token = Token(inner.last_token_id);
        inner.poll.registry().register(source, token, interest)?;

        Ok(token)
    }

    fn deregister<S: mio::event::Source>(&self, source: &mut S) -> io::Result<()> {
        let inner = self.inner.lock().unwrap();
        inner.poll.registry().deregister(source)
    }

    fn register_waker(&self, token: Token, waker: Waker) {
        let mut inner = self.inner.lock().unwrap();
        inner.wakers.insert(token, waker);
    }
}

trait AsyncRead {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>>;

    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadFuture<'a, Self>
    where
        Self: Unpin,
    {
        ReadFuture { reader: self, buf }
    }
}

struct ReadFuture<'a, R: ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
}

impl<'a, R: AsyncRead + Unpin + ?Sized> Future for ReadFuture<'a, R> {
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let ReadFuture { reader, buf } = &mut *self;
        Pin::new(&mut **reader).poll_read(cx, buf)
    }
}

trait AsyncWrite {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>>;

    fn write<'a>(&'a mut self, buf: &'a [u8]) -> WriteFuture<'a, Self>
    where
        Self: Unpin,
    {
        WriteFuture {
            writer: self,
            buf,
            pos: 0,
        }
    }
}

struct WriteFuture<'a, W: ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
    pos: usize,
}

impl<'a, W: AsyncWrite + Unpin + ?Sized> Future for WriteFuture<'a, W> {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { writer, buf, pos } = &mut *self;

        while *pos < buf.len() {
            let n = match Pin::new(&mut **writer).poll_write(cx, &buf[*pos..]) {
                Poll::Ready(Ok(n)) => n,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            *pos += n;
        }

        Poll::Ready(Ok(()))
    }
}

struct AsyncTcpListener {
    inner: MioTcpListener,
    token: Token,
    executor: Arc<Executor>,
}

impl AsyncTcpListener {
    fn bind(addr: &str, executor: Arc<Executor>) -> io::Result<Self> {
        let addr = addr.parse().unwrap();
        let mut inner = MioTcpListener::bind(addr)?;
        let token = executor.register(&mut inner, Interest::READABLE)?;

        println!("TcpListener bound to {} with token {:?}", addr, token);

        Ok(Self {
            inner,
            token,
            executor,
        })
    }

    async fn accept(&mut self) -> io::Result<(AsyncTcpStream, SocketAddr)> {
        AcceptFuture {
            listener: &mut self.inner,
            token: self.token,
            executor: self.executor.clone(),
        }
        .await
    }
}

impl Drop for AsyncTcpListener {
    fn drop(&mut self) {
        self.executor.deregister(&mut self.inner).unwrap();
        println!("TcpListener {:?} deregistered", self.token)
    }
}

struct AcceptFuture<'a> {
    listener: &'a mut MioTcpListener,
    token: Token,
    executor: Arc<Executor>,
}

impl<'a> Future for AcceptFuture<'a> {
    type Output = io::Result<(AsyncTcpStream, SocketAddr)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.listener.accept() {
            Ok((socket, addr)) => {
                println!("Accepted connection from {}", addr);

                match AsyncTcpStream::new(socket, addr, self.executor.clone()) {
                    Ok(async_socket) => Poll::Ready(Ok((async_socket, addr))),
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                self.executor.register_waker(self.token, cx.waker().clone());

                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

struct AsyncTcpStream {
    inner: MioTcpStream,
    token: Token,
    addr: SocketAddr,
    executor: Arc<Executor>,
}

impl AsyncTcpStream {
    fn new(mut inner: MioTcpStream, addr: SocketAddr, executor: Arc<Executor>) -> io::Result<Self> {
        let token = executor.register(&mut inner, Interest::READABLE | Interest::WRITABLE)?;

        Ok(Self {
            inner,
            token,
            addr,
            executor,
        })
    }
}

impl AsyncRead for AsyncTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.inner.read(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                self.executor.register_waker(self.token, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncWrite for AsyncTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.inner.write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                self.executor.register_waker(self.token, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl Drop for AsyncTcpStream {
    fn drop(&mut self) {
        self.executor.deregister(&mut self.inner).unwrap();
        println!("Connection {:?} deregistered", self.addr)
    }
}

async fn handle_conn(mut socket: AsyncTcpStream) -> io::Result<()> {
    let mut buffer = [0; 1024];
    loop {
        let n = socket.read(&mut buffer).await?;
        if n == 0 {
            break;
        }

        println!(
            "Read {} bytes ({})",
            n,
            String::from_utf8_lossy(&buffer[..n]).trim_end()
        );

        socket.write(&buffer[..n]).await?;
        println!(
            "Echoed {} bytes ({})",
            n,
            String::from_utf8_lossy(&buffer[..n]).trim_end()
        );
    }
    Ok(())
}

async fn echo_server(executor: Arc<Executor>) -> io::Result<()> {
    let mut listener = AsyncTcpListener::bind("127.0.0.1:10000", executor.clone())?;

    loop {
        let (socket, _) = listener.accept().await?;

        executor.spawn(async move {
            let _ = handle_conn(socket).await;
        });
    }
}

fn main() {
    let executor = Arc::new(Executor::new().unwrap());
    let _ = executor.run(echo_server(executor.clone()));
}
