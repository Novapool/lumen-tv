"""Tests for session/lumen-soloist, run under dash with a temp HOME and stub curl/systemctl/uname/stty."""
import io
import os
import pty
import select
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "lumen-soloist")
SHELL = shutil.which("dash") or "/bin/dash"
KEY = "SECRETKEY-abc123-DO-NOT-LEAK"

CURL = """#!/bin/sh
echo "curl $*" >> "$STUB_LOG"
[ -z "${STUB_CURL_FAIL:-}" ] || exit 22
while [ $# -gt 0 ]; do
    if [ "$1" = "-o" ]; then cp "$STUB_TARBALL" "$2"; fi
    shift
done
"""
SYSTEMCTL = """#!/bin/sh
echo "systemctl $*" >> "$STUB_LOG"
case "$*" in
  *is-active*) if [ -f "$STUB_ACTIVE" ]; then echo active; exit 0; else echo inactive; exit 3; fi ;;
  *is-enabled*) echo enabled ;;
  *"show"*ExecMainStatus*) echo "${STUB_EXIT:-0}" ;;
esac
exit 0
"""
UNAME = """#!/bin/sh
echo "${STUB_ARCH:-aarch64}"
"""
STTY = """#!/bin/sh
echo "stty $*" >> "$STUB_LOG"
[ "$1" = "-g" ] && echo "saved-settings"
exit 0
"""

ELF_AARCH64 = b"\x7fELF\x02\x01\x01" + b"\0" * 9 + b"\x03\x00\xb7\x00" + b"\0" * 40
ELF_X86 = b"\x7fELF\x02\x01\x01" + b"\0" * 9 + b"\x03\x00\x3e\x00" + b"\0" * 40


def make_tarball(path, binary):
    with tarfile.open(path, "w:gz") as t:
        for name, data, mode in (("soloist", binary, 0o555), ("CHANGELOG.md", b"changes\n", 0o644),
                                 ("THIRD_PARTY_LICENSES.txt", b"licenses\n", 0o644)):
            ti = tarfile.TarInfo(name)
            ti.size = len(data)
            ti.mode = mode
            t.addfile(ti, io.BytesIO(data))


class Base(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="lsol-test-")
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.home = os.path.join(self.root, "home")
        self.stubs = os.path.join(self.root, "stubs")
        self.tmp = os.path.join(self.root, "tmp")
        for d in (self.home, self.stubs, self.tmp):
            os.makedirs(d)
        for name, body in (("curl", CURL), ("systemctl", SYSTEMCTL), ("uname", UNAME), ("stty", STTY)):
            p = os.path.join(self.stubs, name)
            with open(p, "w") as f:
                f.write(body)
            os.chmod(p, 0o755)
        self.log = os.path.join(self.root, "stub.log")
        open(self.log, "w").close()
        self.tarball = os.path.join(self.root, "s.tgz")
        make_tarball(self.tarball, ELF_AARCH64)
        self.active = os.path.join(self.root, "active")
        self.env = {
            "HOME": self.home,
            "PATH": self.stubs + ":/usr/bin:/bin",
            "TMPDIR": self.tmp,
            "XDG_RUNTIME_DIR": os.path.join(self.root, "run"),
            "STUB_LOG": self.log,
            "STUB_TARBALL": self.tarball,
            "STUB_ACTIVE": self.active,
        }

    # helpers
    def run_script(self, *args, stdin=b"", env=None):
        e = dict(self.env)
        e.update(env or {})
        return subprocess.run([SHELL, SCRIPT, *args], input=stdin, capture_output=True, env=e, timeout=60)

    @staticmethod
    def rd(path, mode="r"):
        with open(path, mode) as f:
            return f.read()

    def calls(self):
        with open(self.log) as f:
            return f.read().splitlines()

    def clear_log(self):
        open(self.log, "w").close()

    def p(self, *parts):
        return os.path.join(self.home, *parts)

    def unit(self, name="soloist.service"):
        with open(self.p(".config/systemd/user", name)) as f:
            return f.read()

    def set_active(self, on):
        if on:
            open(self.active, "w").close()
        elif os.path.exists(self.active):
            os.remove(self.active)

    def put_key(self):
        os.makedirs(self.p(".config/soloist"), mode=0o700)
        with open(self.p(".config/soloist/key"), "w") as f:
            f.write(KEY + "\n")

    def installed(self, **kw):
        r = self.run_script("install", **kw)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.clear_log()
        return r

    def restarted(self):
        return any(c == "systemctl --user restart soloist.service" for c in self.calls())

    def tmp_clean(self):
        self.assertEqual(os.listdir(self.tmp), [])


class InstallTests(Base):
    def test_writes_units_and_enables_timer(self):
        r = self.run_script("install")
        self.assertEqual(r.returncode, 0, r.stderr)
        svc = self.unit()
        self.assertIn("-n %H", svc)
        self.assertIn("-w 127.0.0.1:9090", svc)
        self.assertIn("-z 300", svc)
        self.assertIn("ConditionPathExists=%h/.config/soloist/key", svc)
        self.assertIn("RestartPreventExitStatus=10", svc)
        self.assertIn("wlan0", svc)
        self.assertIn("scope global", svc)
        self.assertNotIn(KEY, svc)
        upd = self.unit("soloist-update.service")
        self.assertIn(f'ExecStart=/bin/sh "{SCRIPT}" update', upd)
        tim = self.unit("soloist-update.timer")
        self.assertIn("OnCalendar=weekly", tim)
        self.assertIn("Persistent=true", tim)
        self.assertIn("RandomizedDelaySec=", tim)
        c = self.calls()
        self.assertIn("systemctl --user daemon-reload", c)
        self.assertIn("systemctl --user enable --now soloist-update.timer", c)
        self.assertIn("systemctl --user enable soloist.service", c)
        curl = [x for x in c if x.startswith("curl")][0]
        self.assertIn("--proto =https", curl)
        self.assertIn("https://soloist-builds.spotifycdn.com/soloist_release_arm64.tar.gz", curl)
        b = self.p(".local/lib/soloist/soloist")
        self.assertTrue(os.access(b, os.X_OK))
        self.assertTrue(os.path.exists(self.p(".local/lib/soloist/THIRD_PARTY_LICENSES.txt")))
        self.tmp_clean()

    def test_without_key_does_not_start(self):
        r = self.run_script("install")
        self.assertIn(b"waiting for a key", r.stdout)
        self.assertFalse([c for c in self.calls() if " start " in c or " restart " in c])

    def test_with_key_starts(self):
        self.put_key()
        r = self.run_script("install")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("systemctl --user start soloist.service", self.calls())

    def test_adoption_unchanged_does_not_restart(self):
        self.put_key()
        self.installed()
        self.set_active(True)
        r = self.run_script("install")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertFalse(self.restarted())
        self.assertIn(b"not restarted", r.stdout)

    def test_adoption_changed_unit_restarts(self):
        self.put_key()
        self.installed()
        with open(self.p(".config/systemd/user/soloist.service"), "a") as f:
            f.write("# hand edit\n")
        self.set_active(True)
        r = self.run_script("install")
        self.assertTrue(self.restarted())
        self.assertIn(b"restarted", r.stdout)

    def test_adoption_changed_binary_restarts(self):
        self.put_key()
        self.installed()
        make_tarball(self.tarball, ELF_AARCH64 + b"new")
        self.set_active(True)
        self.run_script("install")
        self.assertTrue(self.restarted())

    def test_replaces_read_only_binary(self):
        # pitv's hand-made install has a 0555 binary
        self.installed()
        os.chmod(self.p(".local/lib/soloist/soloist"), 0o555)
        make_tarball(self.tarball, ELF_AARCH64 + b"v2")
        r = self.run_script("install")
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_non_aarch64_machine_refused(self):
        r = self.run_script("install", env={"STUB_ARCH": "x86_64"})
        self.assertNotEqual(r.returncode, 0)
        self.assertIn(b"aarch64", r.stderr)
        self.assertFalse(os.path.exists(self.p(".local/lib/soloist")))

    def test_non_aarch64_binary_refused(self):
        make_tarball(self.tarball, ELF_X86)
        r = self.run_script("install")
        self.assertNotEqual(r.returncode, 0)
        self.assertFalse(os.path.exists(self.p(".local/lib/soloist/soloist")))
        self.tmp_clean()

    def test_download_failure_keeps_old_binary(self):
        self.installed()
        before = self.rd(self.p(".local/lib/soloist/soloist"), "rb")
        r = self.run_script("install", env={"STUB_CURL_FAIL": "1"})
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.rd(self.p(".local/lib/soloist/soloist"), "rb"), before)
        self.assertEqual(len(r.stderr.strip().splitlines()), 1)
        self.tmp_clean()

    def test_curl_has_stall_limits_and_update_unit_timeout(self):
        self.run_script("install")
        curl = [x for x in self.calls() if x.startswith("curl")][0]
        self.assertIn("--speed-limit 1024", curl)
        self.assertIn("--speed-time 60", curl)
        self.assertIn("--max-time 900", curl)
        self.assertIn("TimeoutStartSec=15min", self.unit("soloist-update.service"))

    def test_arm64_machine_refused(self):
        r = self.run_script("install", env={"STUB_ARCH": "arm64"})
        self.assertNotEqual(r.returncode, 0)


class UpdateTests(Base):
    def test_same_binary_does_nothing(self):
        self.put_key()
        self.installed()
        self.set_active(True)
        r = self.run_script("update")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertFalse(self.restarted())
        self.assertIn(b"unchanged, no restart", r.stdout)

    def test_update_requires_install(self):
        r = self.run_script("update")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn(b"not installed (run install first)", r.stderr)
        self.assertEqual([c for c in self.calls() if c.startswith("curl")], [])

    def test_new_binary_restarts_when_active(self):
        self.installed()
        self.set_active(True)
        make_tarball(self.tarball, ELF_AARCH64 + b"v2")
        r = self.run_script("update")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(self.restarted())
        self.assertTrue(self.rd(self.p(".local/lib/soloist/soloist"), "rb").endswith(b"v2"))

    def test_new_binary_restarts_after_expiry_exit(self):
        self.installed()
        make_tarball(self.tarball, ELF_AARCH64 + b"v2")
        self.run_script("update", env={"STUB_EXIT": "10"})
        self.assertTrue(self.restarted())

    def test_new_binary_leaves_stopped_unit_stopped(self):
        self.installed()
        make_tarball(self.tarball, ELF_AARCH64 + b"v2")
        self.run_script("update", env={"STUB_EXIT": "0"})
        self.assertFalse(self.restarted())

    def test_force_restarts_when_unchanged(self):
        self.installed()
        self.set_active(True)
        r = self.run_script("update", "--force")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(self.restarted())

    def test_failed_download_keeps_binary_and_fails(self):
        self.installed()
        self.set_active(True)
        before = self.rd(self.p(".local/lib/soloist/soloist"), "rb")
        r = self.run_script("update", env={"STUB_CURL_FAIL": "1"})
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.rd(self.p(".local/lib/soloist/soloist"), "rb"), before)
        self.assertFalse(self.restarted())
        self.tmp_clean()


class UninstallTests(Base):
    def setUp(self):
        super().setUp()
        self.put_key()
        os.makedirs(self.p(".local/share/soloist"))
        with open(self.p(".local/share/soloist/.device_id"), "w") as f:
            f.write("x")
        self.installed()

    def unit_files(self):
        d = self.p(".config/systemd/user")
        return [f for f in os.listdir(d) if f.startswith("soloist")] if os.path.isdir(d) else []

    def test_removes_units_and_binary_keeps_key_and_state(self):
        r = self.run_script("uninstall")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.unit_files(), [])
        self.assertFalse(os.path.exists(self.p(".local/lib/soloist")))
        self.assertTrue(os.path.exists(self.p(".config/soloist/key")))
        self.assertTrue(os.path.exists(self.p(".local/share/soloist/.device_id")))
        c = self.calls()
        self.assertIn("systemctl --user stop soloist.service", c)
        self.assertIn("systemctl --user disable soloist.service soloist-update.timer", c)
        self.assertIn("systemctl --user daemon-reload", c)
        self.assertTrue([x for x in c if "reset-failed" in x])
        self.assertIn(b"kept your key", r.stdout)

    def test_stops_update_service_and_purge_mentions_cache(self):
        self.clear_log()
        r = self.run_script("uninstall", "--purge")
        self.assertIn("systemctl --user stop soloist-update.service", self.calls())
        self.assertIn(b"cache", r.stdout)

    def test_purge_removes_key_and_state(self):
        r = self.run_script("uninstall", "--purge")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertFalse(os.path.exists(self.p(".config/soloist")))
        self.assertFalse(os.path.exists(self.p(".local/share/soloist")))

    def test_second_uninstall_succeeds(self):
        self.run_script("uninstall")
        r = self.run_script("uninstall")
        self.assertEqual(r.returncode, 0, r.stderr)
        r = self.run_script("uninstall", "--purge")
        self.assertEqual(r.returncode, 0, r.stderr)


class SetKeyTests(Base):
    def setUp(self):
        super().setUp()
        self.installed()

    def assert_no_leak(self, r):
        self.assertNotIn(KEY.encode(), r.stdout)
        self.assertNotIn(KEY.encode(), r.stderr)
        self.assertNotIn(KEY, "\n".join(self.calls()))

    def test_writes_key_with_modes(self):
        r = self.run_script("set-key", stdin=f"  {KEY} \r\n".encode())
        self.assertEqual(r.returncode, 0, r.stderr)
        k = self.p(".config/soloist/key")
        self.assertEqual(self.rd(k), KEY + "\n")
        self.assertEqual(stat.S_IMODE(os.stat(k).st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(os.stat(os.path.dirname(k)).st_mode), 0o700)
        self.assertEqual(os.listdir(os.path.dirname(k)), ["key"])
        self.assert_no_leak(r)
        self.assertIn("systemctl --user enable soloist.service", self.calls())
        self.assertTrue(self.restarted())
        self.assertNotIn("stty", " ".join(self.calls()))  # not a terminal

    def test_no_trailing_newline_ok(self):
        r = self.run_script("set-key", stdin=KEY.encode())
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.rd(self.p(".config/soloist/key")), KEY + "\n")

    def test_refuses_empty(self):
        for data in (b"", b"\n", b"   \n"):
            r = self.run_script("set-key", stdin=data)
            self.assertNotEqual(r.returncode, 0)
        self.assertFalse(os.path.exists(self.p(".config/soloist/key")))

    def test_refuses_multiline(self):
        r = self.run_script("set-key", stdin=f"{KEY}\nsecond-line\n".encode())
        self.assertNotEqual(r.returncode, 0)
        self.assertFalse(os.path.exists(self.p(".config/soloist/key")))
        self.assert_no_leak(r)

    def test_refuses_when_not_installed(self):
        self.run_script("uninstall")
        r = self.run_script("set-key", stdin=KEY.encode())
        self.assertNotEqual(r.returncode, 0)
        self.assertFalse(os.path.exists(self.p(".config/soloist/key")))

    def test_terminal_turns_echo_off_and_restores(self):
        master, slave = pty.openpty()
        e = dict(self.env)
        proc = subprocess.Popen([SHELL, SCRIPT, "set-key"], stdin=slave, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, env=e)
        os.close(slave)
        os.write(master, (KEY + "\n").encode())
        out, err = proc.communicate(timeout=30)
        os.close(master)
        self.assertEqual(proc.returncode, 0, err)
        calls = self.calls()
        self.assertIn("stty -g", calls)
        self.assertIn("stty -echo", calls)
        self.assertIn("stty saved-settings", calls)
        self.assertEqual(self.rd(self.p(".config/soloist/key")), KEY + "\n")
        self.assertNotIn(KEY.encode(), out + err)


class OtherTests(Base):
    def test_dry_run_changes_nothing(self):
        for cmd in (["install"], ["update"], ["update", "--force"], ["uninstall"], ["uninstall", "--purge"], ["set-key"]):
            r = self.run_script("--dry-run", *cmd)
            self.assertEqual(r.returncode, 0, (cmd, r.stderr))
        self.assertEqual(os.listdir(self.home), [])
        self.assertEqual(self.calls(), [])
        self.tmp_clean()

    def test_dry_run_install_shows_plan(self):
        r = self.run_script("install", "--dry-run", env={"STUB_ARCH": "x86_64"})
        self.assertEqual(r.returncode, 0, r.stderr)
        out = r.stdout.decode()
        self.assertIn("soloist_release_arm64.tar.gz", out)
        self.assertIn("-n %H", out)
        self.assertIn("OnCalendar=weekly", out)
        self.assertIn("systemctl --user enable --now soloist-update.timer", out)

    def test_dry_run_does_not_delete(self):
        self.put_key()
        self.installed()
        self.run_script("--dry-run", "uninstall", "--purge")
        self.assertTrue(os.path.exists(self.p(".local/lib/soloist/soloist")))
        self.assertTrue(os.path.exists(self.p(".config/soloist/key")))

    def test_status_never_prints_key(self):
        self.put_key()
        r = self.run_script("status")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn(b"key: present", r.stdout)
        self.assertNotIn(KEY.encode(), r.stdout + r.stderr)

    def test_force_and_purge_only_where_they_apply(self):
        for args in (["install", "--force"], ["uninstall", "--force"], ["install", "--purge"], ["update", "--purge"]):
            r = self.run_script(*args)
            self.assertNotEqual(r.returncode, 0, args)
            self.assertIn(b"only applies to", r.stderr)
        self.assertEqual(os.listdir(self.home), [])

    def test_no_systemctl_status_suggestion(self):
        self.assertNotIn("systemctl --user status", self.rd(SCRIPT))

    def test_unknown_command_fails(self):
        self.assertNotEqual(self.run_script("frobnicate").returncode, 0)
        self.assertNotEqual(self.run_script().returncode, 0)

    def test_syntax_clean(self):
        r = subprocess.run([SHELL, "-n", SCRIPT], capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr)


if __name__ == "__main__":
    unittest.main()
