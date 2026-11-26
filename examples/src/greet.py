"""Tiny module used by the marklark examples."""

# START greet
def greet(name: str) -> str:
    """Return a friendly greeting."""
    return f"Hello, {name}!"
# END greet


def shout(name: str) -> str:
    """Return the greeting in capitals."""
    return greet(name).upper()
