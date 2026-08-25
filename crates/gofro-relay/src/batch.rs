use std::{
    io,
    net::{SocketAddr, UdpSocket},
};

use crate::codec::BUFFER_SIZE;

pub(crate) const BATCH_SIZE: usize = 16;
pub(crate) type PacketBatch = [[u8; BUFFER_SIZE]; BATCH_SIZE];
pub(crate) type PacketLengths = [usize; BATCH_SIZE];
pub(crate) type PacketSources = [Option<SocketAddr>; BATCH_SIZE];

#[cfg(target_os = "linux")]
fn message_headers() -> [libc::mmsghdr; BATCH_SIZE] {
    std::array::from_fn(|_| {
        // SAFETY: null pointers and zero lengths/flags are a valid empty C message header.
        unsafe { std::mem::MaybeUninit::<libc::mmsghdr>::zeroed().assume_init() }
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn recv_many(
    socket: &UdpSocket,
    buffers: &mut PacketBatch,
    lengths: &mut PacketLengths,
) -> io::Result<usize> {
    recv_impl(socket, buffers, lengths, None)
}

#[cfg(target_os = "linux")]
pub(crate) fn recv_from_many(
    socket: &UdpSocket,
    buffers: &mut PacketBatch,
    lengths: &mut PacketLengths,
    sources: &mut PacketSources,
) -> io::Result<usize> {
    recv_impl(socket, buffers, lengths, Some(sources))
}

#[cfg(target_os = "linux")]
fn recv_impl(
    socket: &UdpSocket,
    buffers: &mut PacketBatch,
    lengths: &mut PacketLengths,
    sources: Option<&mut PacketSources>,
) -> io::Result<usize> {
    use std::{array, os::fd::AsRawFd, ptr};

    use socket2::{SockAddr, SockAddrStorage};

    let mut iovecs: [libc::iovec; BATCH_SIZE] = array::from_fn(|index| libc::iovec {
        iov_base: buffers[index].as_mut_ptr().cast(),
        iov_len: BUFFER_SIZE,
    });
    let mut messages = message_headers();
    for (message, iovec) in messages.iter_mut().zip(&mut iovecs) {
        message.msg_hdr.msg_iov = iovec;
        message.msg_hdr.msg_iovlen = 1;
    }
    let mut addresses: Option<[SockAddrStorage; BATCH_SIZE]> = sources
        .as_ref()
        .map(|_| array::from_fn(|_| SockAddrStorage::zeroed()));
    if let Some(addresses) = addresses.as_mut() {
        for (message, address) in messages.iter_mut().zip(addresses) {
            // SAFETY: SockAddrStorage is transparent over libc::sockaddr_storage.
            message.msg_hdr.msg_name =
                unsafe { ptr::from_mut(address.view_as::<libc::sockaddr_storage>()).cast() };
            message.msg_hdr.msg_namelen = address.size_of();
        }
    }

    let received = loop {
        // SAFETY: every message points to one distinct, live buffer of BUFFER_SIZE bytes.
        let result = unsafe {
            libc::recvmmsg(
                socket.as_raw_fd(),
                messages.as_mut_ptr(),
                BATCH_SIZE as libc::c_uint,
                libc::MSG_WAITFORONE as _,
                ptr::null_mut(),
            )
        };
        if result >= 0 {
            break result as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    for (length, message) in lengths.iter_mut().zip(&messages).take(received) {
        *length = message.msg_len as usize;
    }
    if let (Some(mut addresses), Some(sources)) = (addresses, sources) {
        for index in 0..received {
            let length = messages[index].msg_hdr.msg_namelen;
            if length > addresses[index].size_of() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid UDP source address",
                ));
            }
            // SAFETY: recvmmsg initialized the address family and reported a bounded length.
            let storage = std::mem::replace(&mut addresses[index], SockAddrStorage::zeroed());
            let address = unsafe { SockAddr::new(storage, length) };
            sources[index] = address.as_socket();
            if sources[index].is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported UDP source address",
                ));
            }
        }
    }
    Ok(received)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn recv_many(
    socket: &UdpSocket,
    buffers: &mut PacketBatch,
    lengths: &mut PacketLengths,
) -> io::Result<usize> {
    lengths[0] = socket.recv(&mut buffers[0])?;
    Ok(1)
}

#[cfg(not(target_os = "linux"))]
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

#[cfg(target_os = "linux")]
pub(crate) fn send_many(
    socket: &UdpSocket,
    buffers: &PacketBatch,
    lengths: &PacketLengths,
    count: usize,
) -> io::Result<()> {
    use std::{array, os::fd::AsRawFd};

    if count > BATCH_SIZE || lengths[..count].iter().any(|length| *length > BUFFER_SIZE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid UDP batch",
        ));
    }
    let mut iovecs: [libc::iovec; BATCH_SIZE] = array::from_fn(|index| libc::iovec {
        iov_base: buffers[index].as_ptr().cast::<libc::c_void>().cast_mut(),
        iov_len: lengths[index],
    });
    let mut messages = message_headers();
    for (message, iovec) in messages.iter_mut().zip(&mut iovecs).take(count) {
        message.msg_hdr.msg_iov = iovec;
        message.msg_hdr.msg_iovlen = 1;
    }

    let mut sent = 0;
    while sent < count {
        // SAFETY: each remaining message points to one live buffer and a validated length.
        let result = unsafe {
            libc::sendmmsg(
                socket.as_raw_fd(),
                messages[sent..].as_mut_ptr(),
                (count - sent) as libc::c_uint,
                0,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let written = result as usize;
        if written == 0
            || messages[sent..sent + written]
                .iter()
                .zip(&lengths[sent..sent + written])
                .any(|(message, length)| message.msg_len as usize != *length)
        {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "partial UDP send"));
        }
        sent += written;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn send_many(
    socket: &UdpSocket,
    buffers: &PacketBatch,
    lengths: &PacketLengths,
    count: usize,
) -> io::Result<()> {
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
}
