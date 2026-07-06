#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    #[tokio::test]
    async fn test_channel_body_multiple_chunks() {
        let (tx, mut body) = ChannelBody::channel(16);

        let chunk1 = Bytes::from("hello");
        let chunk2 = Bytes::from("world");

        tx.send(chunk1.clone()).await.unwrap();
        tx.send(chunk2.clone()).await.unwrap();
        drop(tx);

        let mut cx = Context::from_waker(std::task::Waker::noop().into());
        let mut body = Pin::new(&mut body);

        let frame1 = hyper::body::Frame::data(chunk1);
        let frame2 = hyper::body::Frame::data(chunk2);

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => assert_eq!(frame, frame1),
            _ => panic!("Expected first frame"),
        }

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => assert_eq!(frame, frame2),
            _ => panic!("Expected second frame"),
        }

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(None) => {}
            _ => panic!("Expected end of stream"),
        }
    }

    #[tokio::test]
    async fn test_channel_body_sender_dropped() {
        let (tx, mut body) = ChannelBody::channel(16);
        drop(tx);

        let mut cx = Context::from_waker(std::task::Waker::noop().into());
        let mut body = Pin::new(&mut body);

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(None) => {}
            _ => panic!("Expected Poll::Ready(None) when sender is dropped"),
        }
    }

    #[tokio::test]
    async fn test_channel_body_empty() {
        let (_tx, mut body) = ChannelBody::channel(16);

        let mut cx = Context::from_waker(std::task::Waker::noop().into());
        let mut body = Pin::new(&mut body);

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Pending => {}
            _ => panic!("Expected Poll::Pending when no data and sender is alive"),
        }
    }

    #[tokio::test]
    async fn test_channel_body_large_chunks() {
        let (tx, mut body) = ChannelBody::channel(64);

        let large_chunk = Bytes::from(vec![0u8; 1000]);
        let frame = hyper::body::Frame::data(large_chunk.clone());
        tx.send(large_chunk).await.unwrap();
        drop(tx);

        let mut cx = Context::from_waker(std::task::Waker::noop().into());
        let mut body = Pin::new(&mut body);

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(f))) => assert_eq!(f, frame),
            _ => panic!("Expected large chunk frame"),
        }

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(None) => {}
            _ => panic!("Expected end of stream"),
        }
    }

    #[tokio::test]
    async fn test_channel_body_multiple_chunks_with_drop() {
        let (tx, mut body) = ChannelBody::channel(16);

        let chunk1 = Bytes::from("first");
        let frame1 = hyper::body::Frame::data(chunk1.clone());
        tx.send(chunk1).await.unwrap();
        drop(tx);

        let mut cx = Context::from_waker(std::task::Waker::noop().into());
        let mut body = Pin::new(&mut body);

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => assert_eq!(frame, frame1),
            _ => panic!("Expected first frame"),
        }

        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(None) => {}
            _ => panic!("Expected end of stream after sender dropped"),
        }
    }
}