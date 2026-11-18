"""Type stubs for the Rust extension. Use the ``lsdocs_plugin`` API instead."""

from collections.abc import Callable
from typing import Any

PROTOCOL_VERSION: str

class Context:
    """What a handler receives alongside its params."""

    @property
    def site(self) -> dict[str, Any]:
        """The site being built: ``title``, ``base_url``, ``root``,
        ``source_dir`` and ``files`` (every source file, relative to the
        source directory)."""

    @property
    def settings(self) -> dict[str, str | None]:
        """Each of the plugin's ``config_files`` (``{name}`` resolved)
        mapped to its text, or None if missing."""

    @property
    def config(self) -> dict[str, str]:
        """``dir`` (the site's ``.config/``) and ``name`` (the plugin's
        name in the site config)."""

    def log(self, level: str, message: str) -> None:
        """Logs at ``"error"``, ``"warn"``, ``"info"``, ``"debug"`` or ``"trace"``."""

    def debug(self, message: str) -> None: ...
    def info(self, message: str) -> None: ...
    def warn(self, message: str) -> None: ...
    def progress(self, message: str, percentage: int | None = None) -> None: ...
    def diagnostic(self, diagnostic: dict[str, Any]) -> None:
        """Reports a diagnostic: ``severity``, ``message``, and optionally
        ``source``, ``span`` and ``code``."""

class NativePlugin:
    def __init__(
        self,
        name: str,
        version: str,
        max_concurrency: int | None = 1,
        config_files: list[str] = ...,
    ) -> None: ...
    def set_parse(self, extensions: list[str], func: Callable[..., Any]) -> None: ...
    def set_read(self, func: Callable[..., Any]) -> None: ...
    def set_apply(self, nodes: list[str], func: Callable[..., Any]) -> None: ...
    def set_render(self, formats: list[str], func: Callable[..., Any]) -> None: ...
    def set_render_all(self, func: Callable[..., Any]) -> None: ...
    def capabilities(self) -> dict[str, Any]: ...

def serve(plugin: NativePlugin) -> None: ...
def call(
    plugin: NativePlugin,
    method: str,
    params: dict[str, Any] | None = None,
    site: dict[str, Any] | None = None,
    settings: Any = None,
    config: dict[str, str] | None = None,
) -> tuple[dict[str, Any], list[dict[str, Any]]]: ...
