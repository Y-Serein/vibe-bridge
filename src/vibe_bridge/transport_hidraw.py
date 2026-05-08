"""Real HID transport over a ``/dev/hidraw*`` character device.

The Vibe HID gadget on the board declares a vendor report length of 64 bytes
(see ``buildroot/board/cvitek/SG200X/overlay/etc/init.d/S08usbdev``). The Linux
``hidraw`` driver gives us per-report framing for free, so we do **not** add a
length prefix — every ``os.read`` returns exactly one HID report's worth of
bytes (up to ``report_length``) and every ``os.write`` is taken as one outbound
report.

Wire format is identical to ``MockHidClient`` / ``MockHidServer`` (the same
``Packet`` codec); only the framing differs. The first byte of every report is
the report id, which our packet header already carries.

The daemon uses this transport in ``daemon --hidraw /dev/hidraw0`` mode, while
the ``hid`` CLI subcommands use it for standalone probing and handshake checks.
"""

from __future__ import annotations

import errno
import os
import select
import threading
from dataclasses import dataclass
from glob import glob
from typing import List, Optional

from .hid_protocol import HEADER_SIZE, Packet, ProtocolError
from .transport import Transport, TransportClosed

DEFAULT_REPORT_LENGTH = 64


@dataclass
class HidrawDeviceInfo:
    path: str
    vid: Optional[int] = None
    pid: Optional[int] = None
    name: Optional[str] = None
    readable: bool = False
    writable: bool = False
    err: Optional[str] = None

    def vid_pid_str(self) -> str:
        if self.vid is None or self.pid is None:
            return "??:??"
        return f"{self.vid:04x}:{self.pid:04x}"


def _read_text(path: str) -> Optional[str]:
    try:
        with open(path, "r", encoding="utf-8") as f:
            return f.read().strip()
    except OSError:
        return None


def _parse_id_pair(s: Optional[str]) -> Optional[int]:
    if s is None:
        return None
    try:
        return int(s, 16)
    except ValueError:
        return None


def list_hidraw_devices() -> List[HidrawDeviceInfo]:
    """Enumerate ``/dev/hidraw*`` nodes and best-effort fetch VID/PID from sysfs."""
    out: List[HidrawDeviceInfo] = []
    for dev in sorted(glob("/dev/hidraw*")):
        info = HidrawDeviceInfo(path=dev)
        info.readable = os.access(dev, os.R_OK)
        info.writable = os.access(dev, os.W_OK)

        # /sys/class/hidraw/hidraw0/device/{uevent, ../../idVendor, ../../idProduct}
        # The HID parent typically has a sysfs path like
        #   /sys/class/hidraw/hidraw0/device  (HID device)
        # whose ``modalias`` carries v<VID>p<PID>.
        sysfs = f"/sys/class/hidraw/{os.path.basename(dev)}/device"
        modalias = _read_text(os.path.join(sysfs, "modalias")) or ""
        # modalias example: hid:b0003g0001v0000359Fp00002120
        v_idx = modalias.find("v")
        p_idx = modalias.find("p", v_idx + 1) if v_idx >= 0 else -1
        if v_idx >= 0 and p_idx > v_idx:
            info.vid = _parse_id_pair(modalias[v_idx + 1 : v_idx + 9])
            info.pid = _parse_id_pair(modalias[p_idx + 1 : p_idx + 9])

        info.name = _read_text(os.path.join(sysfs, "uevent"))
        out.append(info)
    return out


class HidrawTransport(Transport):
    """``Transport`` backed by a ``/dev/hidraw*`` device."""

    def __init__(
        self,
        device_path: str,
        *,
        report_length: int = DEFAULT_REPORT_LENGTH,
        nonblocking: bool = True,
    ) -> None:
        flags = os.O_RDWR
        if nonblocking:
            flags |= os.O_NONBLOCK
        self._fd = os.open(device_path, flags)
        self._device_path = device_path
        self._report_length = report_length
        self._lock = threading.Lock()

    @property
    def device_path(self) -> str:
        return self._device_path

    @property
    def report_length(self) -> int:
        return self._report_length

    # -------------------------------------------------------------- Transport

    def send_packet(self, packet: Packet) -> None:
        raw = packet.encode()
        if len(raw) > self._report_length:
            raise ProtocolError(
                f"packet too large for HID report: {len(raw)} > {self._report_length} "
                "(caller must fragment via stream_iter_packets / fragment_payload)"
            )
        # Linux hidraw expects writes to be exactly the report length declared
        # by the descriptor; pad with zeros so undersized headers don't trip
        # the kernel.
        if len(raw) < self._report_length:
            raw = raw + b"\x00" * (self._report_length - len(raw))
        with self._lock:
            written = os.write(self._fd, raw)
            if written != len(raw):
                raise OSError(f"short hidraw write: {written}/{len(raw)}")

    def recv_report(self, timeout: Optional[float] = None) -> Optional[bytes]:
        """Read one raw hidraw report.

        Kept separate from ``recv_packet`` so probe tooling can print the exact
        bytes when the board and host disagree about packet layout.
        """
        rlist, _, _ = select.select([self._fd], [], [], timeout)
        if not rlist:
            return None
        try:
            data = os.read(self._fd, self._report_length)
        except OSError as exc:
            if exc.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                return None
            if exc.errno == errno.ENODEV:
                raise TransportClosed(f"{self._device_path} disappeared") from exc
            raise
        if not data:
            raise TransportClosed(f"{self._device_path} returned EOF")
        return data

    def recv_packet(self, timeout: Optional[float] = None) -> Optional[Packet]:
        data = self.recv_report(timeout)
        if data is None:
            return None
        if len(data) < HEADER_SIZE:
            raise ProtocolError(
                f"hidraw read shorter than packet header: {len(data)} < {HEADER_SIZE}"
            )
        return Packet.decode(data)

    def close(self) -> None:
        if self._fd is None:
            return
        try:
            os.close(self._fd)
        except OSError:
            pass
        self._fd = None  # type: ignore[assignment]
