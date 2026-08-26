use std::{
    io,
    net::{SocketAddr, UdpSocket},
};

use crate::codec::BUFFER_SIZE;

pub(crate) const BATCH_SIZE: usize = 16;
pub(crate) type PacketBatch = [[u8; BUFFER_SIZE]; BATCH_SIZE];
pub(crate) type PacketLengths = [usize; BATCH_SIZE];
pub(crate) type PacketSources = [Option<SocketAddr>; BATCH_SIZE];

pub(crate) fn recv_many(
    socket: &UdpSocket,
    buffers: &mut PacketBatch,
    lengths: &mut PacketLengths,
) -> io::Result<usize> {
    lengths[0] = socket.recv(&mut buffers[0])?;
    Ok(1)
}

pub(crate) fn recv_from_many(
    socket: &UdpSocket,
    buffers: &mut PacketBatch,
    lengths: &mut PacketLengths,
    sources: &mut PacketSources,
) -> io::Result<usize> {
    let (length, source) = socket.recv_from(&mut buffers[0])?;
    lengths[0] = length;
    sources[0] = Some(source);
    Ok(1)
}

pub(crate) fn send_many(
    socket: &UdpSocket,
    buffers: &PacketBatch,
    lengths: &PacketLengths,
    count: usize,
) -> io::Result<()> {
    if count > BATCH_SIZE || lengths[..count].iter().any(|length| *length > BUFFER_SIZE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid UDP batch",
        ));
    }
    for (buffer, length) in buffers.iter().zip(lengths).take(count) {
        if socket.send(&buffer[..*length])? != *length {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "partial UDP send"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_datagram_order() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.connect(receiver.local_addr().unwrap()).unwrap();
        receiver.connect(sender.local_addr().unwrap()).unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();

        let mut sent = [[0; BUFFER_SIZE]; BATCH_SIZE];
        let mut sent_lengths = [0; BATCH_SIZE];
        for (index, message) in [
            b"first".as_slice(),
            b"second".as_slice(),
            b"third".as_slice(),
        ]
        .into_iter()
        .enumerate()
        {
            sent[index][..message.len()].copy_from_slice(message);
            sent_lengths[index] = message.len();
        }
        send_many(&sender, &sent, &sent_lengths, 3).unwrap();

        let mut received = [[0; BUFFER_SIZE]; BATCH_SIZE];
        let mut received_lengths = [0; BATCH_SIZE];
        let mut messages = Vec::new();
        while messages.len() < 3 {
            let count = recv_many(&receiver, &mut received, &mut received_lengths).unwrap();
            messages.extend(
                received[..count]
                    .iter()
                    .zip(&received_lengths[..count])
                    .map(|(packet, length)| packet[..*length].to_vec()),
            );
        }
        assert_eq!(
            messages,
            vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()]
        );
    }

    #[test]
    fn rejects_invalid_batches() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(socket.local_addr().unwrap()).unwrap();
        let buffers = [[0; BUFFER_SIZE]; BATCH_SIZE];
        let mut lengths = [0; BATCH_SIZE];

        assert_eq!(
            send_many(&socket, &buffers, &lengths, BATCH_SIZE + 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        lengths[0] = BUFFER_SIZE + 1;
        assert_eq!(
            send_many(&socket, &buffers, &lengths, 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
