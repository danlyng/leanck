//! Private-socket frames, implemented by Tokio's length codec.
//! Keep each framed reader for the connection lifetime: it owns read-ahead bytes.
use bytes::{Bytes, BytesMut};
use futures::{Sink, SinkExt, Stream, StreamExt};
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, FramedRead, FramedWrite, LengthDelimitedCodec};

pub const MAX_FRAME: usize = 16 * 1024 * 1024;
fn codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .big_endian()
        .length_field_type::<u32>()
        .max_frame_length(MAX_FRAME)
        .new_codec()
}
pub fn framed<T: AsyncRead + AsyncWrite>(io: T) -> Framed<T, LengthDelimitedCodec> {
    Framed::new(io, codec())
}
pub fn reader<R: AsyncRead>(io: R) -> FramedRead<R, LengthDelimitedCodec> {
    FramedRead::new(io, codec())
}
pub fn writer<W: AsyncWrite>(io: W) -> FramedWrite<W, LengthDelimitedCodec> {
    FramedWrite::new(io, codec())
}
pub async fn write_frame<S: Sink<Bytes, Error = io::Error> + Unpin>(
    sink: &mut S,
    body: &[u8],
) -> io::Result<()> {
    if body.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame exceeds limit",
        ));
    }
    sink.send(Bytes::copy_from_slice(body)).await
}
pub async fn read_frame<S: Stream<Item = io::Result<BytesMut>> + Unpin>(
    stream: &mut S,
) -> io::Result<Option<Vec<u8>>> {
    stream
        .next()
        .await
        .transpose()
        .map(|frame| frame.map(|bytes| bytes.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn frame_is_big_endian_length_followed_by_json() {
        let body = br#"{"id":"contract","op":"health","params":{}}"#;
        let mut encoded = Vec::new();
        write_frame(&mut writer(&mut encoded), body).await.unwrap();
        assert_eq!(&encoded[..4], &[0, 0, 0, body.len() as u8]);
        assert_eq!(&encoded[4..], body);
        let mut input = reader(encoded.as_slice());
        assert_eq!(read_frame(&mut input).await.unwrap().unwrap(), body);
        assert!(read_frame(&mut input).await.unwrap().is_none());
    }
    #[tokio::test]
    async fn oversized_input_and_output_have_the_same_limit() {
        let prefix = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert_eq!(
            read_frame(&mut reader(prefix.as_slice()))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let mut encoded = Vec::new();
        assert_eq!(
            write_frame(&mut writer(&mut encoded), &vec![0; MAX_FRAME + 1])
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(encoded.is_empty());
        let body = vec![7; MAX_FRAME];
        write_frame(&mut writer(&mut encoded), &body).await.unwrap();
        assert_eq!(
            read_frame(&mut reader(encoded.as_slice()))
                .await
                .unwrap()
                .unwrap(),
            body
        );
    }
    #[tokio::test]
    async fn truncated_header_and_body_are_errors_and_clean_eof_is_empty() {
        for bytes in [vec![0], vec![0, 0], vec![0, 0, 0], vec![0, 0, 0, 2, 1]] {
            assert!(read_frame(&mut reader(bytes.as_slice())).await.is_err());
        }
        assert!(
            read_frame(&mut reader([].as_slice()))
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn one_reader_preserves_back_to_back_frames_and_partial_delivery() {
        let mut encoded = Vec::new();
        {
            let mut output = writer(&mut encoded);
            write_frame(&mut output, b"one").await.unwrap();
            write_frame(&mut output, b"two").await.unwrap();
        }
        let mut input = reader(encoded.as_slice());
        assert_eq!(read_frame(&mut input).await.unwrap().unwrap(), b"one");
        assert_eq!(read_frame(&mut input).await.unwrap().unwrap(), b"two");
        assert!(read_frame(&mut input).await.unwrap().is_none());
        let (input, mut output) = tokio::io::duplex(1);
        let sender = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            output.write_all(&encoded).await.unwrap();
        });
        let mut input = reader(input);
        assert_eq!(read_frame(&mut input).await.unwrap().unwrap(), b"one");
        assert_eq!(read_frame(&mut input).await.unwrap().unwrap(), b"two");
        sender.await.unwrap();
    }
}
