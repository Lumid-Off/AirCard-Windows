"""Independent TLS 1.2 RSA/CBC + mandatory client-auth regression peer.

Uses only Python's standard library; all certificates are synthetic test data.
"""
import pathlib
import socket
import ssl
import sys

directory = pathlib.Path(sys.argv[1])
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version = ssl.TLSVersion.TLSv1_2
context.maximum_version = ssl.TLSVersion.TLSv1_2
context.set_ciphers("AES256-SHA:@SECLEVEL=0")
context.load_cert_chain(str(directory / "device.pem"), str(directory / "key.pem"))
context.load_verify_locations(cafile=str(directory / "root.pem"))
context.verify_mode = ssl.CERT_REQUIRED
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    listener.settimeout(20)
    print(listener.getsockname()[1], flush=True)
    sock, _ = listener.accept()
    sock.settimeout(15)
    with context.wrap_socket(sock, server_side=True) as stream:
        assert stream.cipher()[0] == "AES256-SHA", stream.cipher()
        assert stream.getpeercert(binary_form=True)
        request = stream.recv(4)
        if sys.argv[2] == "accept":
            assert request == b"ping", request
            stream.sendall(b"pong")
        else:
            assert request == b"", "Client sent data before rejecting wrong certificate"
    print("RSA/CBC and client certificate verified", flush=True)
