#!/usr/bin/env python3
import socket
import sys

address = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"

server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind((address, 0))
server.listen(4)
print(server.getsockname()[1], flush=True)

while True:
    connection, _ = server.accept()
    with connection:
        data = connection.recv(4096)
        connection.sendall(data)
