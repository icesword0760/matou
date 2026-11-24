"""Local TLS endpoint and HTTP, HTTPS and SOCKS proxies for the gateway contract."""
import select
import socket
import ssl
import sys
import threading


def listener():
    sock = socket.create_server(("127.0.0.1", 0))
    sock.settimeout(15)
    return sock


def receive_exact(sock, size):
    received = b""
    while len(received) < size:
        chunk = sock.recv(size - len(received))
        assert chunk, received
        received += chunk
    return received


def read_request(sock):
    head = bytearray()
    while not head.endswith(b"\r\n\r\n"):
        head.extend(receive_exact(sock, 1))
        assert len(head) <= 8192
    line, *headers = head.decode().split("\r\n")
    headers = {name.lower(): value for name, value in
               (header.split(": ", 1) for header in headers if header)}
    return line, headers


def respond(sock):
    sock.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")


def relay(client, target_port):
    with socket.create_connection(("127.0.0.1", target_port), timeout=8) as target:
        while True:
            ready = [client] if isinstance(client, ssl.SSLSocket) and client.pending() else []
            if not ready:
                ready, _, _ = select.select([client, target], [], [], 8)
            assert ready, "tunnel stalled"
            for source in ready:
                data = source.recv(16384)
                if not data:
                    return
                (target if source is client else client).sendall(data)


def verify_upstream(sock):
    line, headers = read_request(sock)
    assert line == "GET /v1/models HTTP/1.1", line
    assert headers["host"] == "upstream.fixture.invalid:8443"
    assert headers["x-api-key"] == "synthetic-a"
    assert "proxy-authorization" not in headers
    respond(sock)


def http_proxy(sock, target_port):
    line, headers = read_request(sock)
    assert headers["proxy-authorization"] == "Basic dXNlcjpwQHNz"
    if line.startswith("CONNECT"):
        assert line == "CONNECT upstream.fixture.invalid:8443 HTTP/1.1", line
        assert headers["host"] == "upstream.fixture.invalid:8443"
        assert "x-api-key" not in headers
        sock.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        relay(sock, target_port)
    else:
        assert line == "GET http://upstream.fixture.invalid:8181/v1/models HTTP/1.1", line
        assert headers["host"] == "upstream.fixture.invalid:8181"
        assert headers["x-api-key"] == "synthetic-a"
        respond(sock)


def socks_proxy(sock, target_port):
    assert receive_exact(sock, 3) == b"\x05\x01\x00"
    sock.sendall(b"\x05\x00")
    host = b"upstream.fixture.invalid"
    assert receive_exact(sock, len(host) + 7) == b"\x05\x01\x00\x03" + bytes([len(host)]) + host + (8443).to_bytes(2, "big")
    sock.sendall(b"\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00")
    relay(sock, target_port)


def serve(listener, count, handler, tls=False):
    try:
        with listener:
            for _ in range(count):
                client, _ = listener.accept()
                with client:
                    client.settimeout(8)
                    if tls:
                        with context.wrap_socket(client, server_side=True) as stream:
                            handler(stream)
                    else:
                        handler(client)
    except Exception as error:
        errors.append(error)


context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(sys.argv[1], sys.argv[2])
errors = []
upstream, http, https, socks = [listener() for _ in range(4)]
port = upstream.getsockname()[1]
print(*(sock.getsockname()[1] for sock in (http, https, socks)), flush=True)
workers = [
    threading.Thread(target=serve, args=(upstream, 3, verify_upstream, True)),
    threading.Thread(target=serve, args=(http, 1, lambda sock: http_proxy(sock, port))),
    threading.Thread(target=serve, args=(https, 2, lambda sock: http_proxy(sock, port), True)),
    threading.Thread(target=serve, args=(socks, 1, lambda sock: socks_proxy(sock, port))),
]
for worker in workers:
    worker.start()
for worker in workers:
    worker.join()
if errors:
    raise errors[0]
