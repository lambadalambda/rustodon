#!/usr/bin/env python3
"""Canonical loopback 443 -> task TLS port; preserves native WebSockets."""
import asyncio
from pathlib import Path


async def copy(reader, writer):
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    finally:
        writer.close()


async def connect(reader, writer):
    port = int(Path('/run-fixture/tls/port').read_text())
    remote_reader, remote_writer = await asyncio.open_connection('127.0.0.1', port)
    await asyncio.gather(copy(reader, remote_writer), copy(remote_reader, writer))


async def main():
    server = await asyncio.start_server(connect, '127.0.0.1', 443)
    async with server:
        await server.serve_forever()


if __name__ == '__main__':
    asyncio.run(main())
