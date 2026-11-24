"""Run TLC with the repository's pinned and checksum-verified tools."""

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
VERSION = "1.7.4"
SHA256 = "936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88"
URL = f"https://github.com/tlaplus/tlaplus/releases/download/v{VERSION}/tla2tools.jar"


def tools_jar(path):
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        print(f"Downloading pinned TLA+ tools {VERSION} to {path}", flush=True)
        with urllib.request.urlopen(URL, timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != SHA256:
            raise RuntimeError("Downloaded TLA+ tools failed SHA-256 verification")
        # Atomic replacement avoids leaving a partial JAR after interruption.
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as download:
            temporary = Path(download.name)
            download.write(data)
        try:
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    if hashlib.sha256(path.read_bytes()).hexdigest() != SHA256:
        raise RuntimeError(f"Expected TLA+ tools {VERSION} with SHA-256 {SHA256}: {path}")
    return path.resolve()


def check(java, jar, module, source, config, timeout, expected=None):
    with tempfile.TemporaryDirectory(prefix="magicmtn-tlc-") as directory:
        work = Path(directory)
        (work / f"{module}.tla").write_text(source)
        (work / f"{module}.cfg").write_text(config)
        result = subprocess.run(
            [java, "-XX:+UseParallelGC", "-Xmx1g", "-cp", str(jar), "tlc2.TLC",
             "-workers", "1", "-fp", "0", "-metadir", str(work / "states"),
             "-config", f"{module}.cfg", f"{module}.tla"],
            cwd=work, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, timeout=timeout,
        )
        if expected:
            # TLC's safety violation exit code is 12. Parser errors, runtime
            # errors, crashes, and timeouts must fail, never count as detection.
            passed = (result.returncode == 12 and
                      f"Invariant {expected} is violated." in result.stdout)
        else:
            passed = (result.returncode == 0 and
                      "Model checking completed. No error has been found." in result.stdout)
        if not passed:
            print(result.stdout, flush=True)
            raise RuntimeError(f"Unexpected TLC result (exit {result.returncode})")
        if expected:
            print(f"  Detected {expected}", flush=True)
        else:
            print(result.stdout, flush=True)


def add_tool_arguments(parser):
    default_java = str(Path(os.environ["JAVA_HOME"]) / "bin" / "java") \
        if os.environ.get("JAVA_HOME") else "java"
    parser.add_argument("--java", default=default_java, help="Java executable (Java 11+)")
    parser.add_argument("--tlc-jar", type=Path,
                        default=ROOT / "build" / "formal-tools" / f"tla2tools-{VERSION}.jar",
                        help="Pinned tools JAR; downloaded if missing, always checksum verified")
    parser.add_argument("--mutation-checks", action="store_true",
                        help="Also require counterexamples for deliberate faults")
    parser.add_argument("--timeout", type=int, default=120, help="Seconds per TLC run")


def verified_tools(parser):
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    java = shutil.which(args.java)
    if java is None:
        parser.error("Java not found; install Java 11+ or supply --java /path/to/java")
    java = str(Path(java).resolve())
    # Detect macOS's java launcher stub before downloading anything.
    version = subprocess.run([java, "-version"], capture_output=True, text=True, timeout=10)
    if version.returncode:
        raise RuntimeError("Java runtime unavailable; install Java 11+ or supply --java /path/to/java")
    jar = tools_jar(args.tlc_jar.expanduser())
    return args, java, jar
