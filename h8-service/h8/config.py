"""Configuration management with XDG-compliant paths."""

import os
import tomllib
from pathlib import Path
from typing import Any

APP_NAME = "h8"

DEFAULT_CONFIG = {
    "account": "tommy.falkowski@iem.fraunhofer.de",
    "timezone": "Europe/Berlin",
    "free_slots": {
        "start_hour": 9,
        "end_hour": 17,
        "exclude_weekends": True,
    },
    "people": {},  # Alias -> email mapping, e.g., {'Roman': 'roman.kowalski@example.com'}
}


def get_config_dir() -> Path:
    """Get the config directory, XDG-compliant."""
    xdg_config = os.environ.get("XDG_CONFIG_HOME")
    if xdg_config:
        return Path(xdg_config) / APP_NAME
    return Path.home() / ".config" / APP_NAME


def get_config_path() -> Path:
    """Get the path to config.toml."""
    return get_config_dir() / "config.toml"


def create_default_config() -> None:
    """Create default config file if it doesn't exist."""
    config_path = get_config_path()
    if config_path.exists():
        return

    config_path.parent.mkdir(parents=True, exist_ok=True)

    default_toml = """# h8 configuration

# Default email account
account = "tommy.falkowski@iem.fraunhofer.de"

# Timezone for calendar operations
timezone = "Europe/Berlin"

# Free slots configuration
[free_slots]
# Only consider times between these hours (24h format)
start_hour = 9
end_hour = 17

# Exclude weekends when finding free slots
exclude_weekends = true
"""
    config_path.write_text(default_toml)


def load_config() -> dict[str, Any]:
    """Load configuration from config.toml."""
    config_path = get_config_path()

    if not config_path.exists():
        create_default_config()

    if config_path.exists():
        with open(config_path, "rb") as f:
            user_config = tomllib.load(f)
        # Merge with defaults
        config = DEFAULT_CONFIG.copy()
        config.update(user_config)
        if "free_slots" in user_config:
            config["free_slots"] = {
                **DEFAULT_CONFIG["free_slots"],
                **user_config["free_slots"],
            }
        if "people" in user_config:
            config["people"] = {**DEFAULT_CONFIG["people"], **user_config["people"]}
        return config

    return DEFAULT_CONFIG.copy()


def resolve_person_alias(alias: str) -> str:
    """Resolve a person alias to email address.

    If the alias is found in config, returns the mapped email.
    If the alias looks like an email (contains @), returns it as-is.
    Otherwise, raises a ValueError.

    Args:
        alias: Person alias or email address

    Returns:
        Email address

    Raises:
        ValueError: If alias is not found and doesn't look like an email
    """
    config = get_config()
    people = config.get("people", {})

    # Case-insensitive lookup
    for name, email in people.items():
        if name.lower() == alias.lower():
            return email

    # If it looks like an email, use it directly
    if "@" in alias:
        return alias

    # Not found
    available = ", ".join(people.keys()) if people else "none configured"
    raise ValueError(f"Unknown person alias '{alias}'. Available aliases: {available}")


# Global config instance
_config: dict[str, Any] | None = None


def get_config() -> dict[str, Any]:
    """Get the configuration (lazy loaded)."""
    global _config
    if _config is None:
        _config = load_config()
    return _config
