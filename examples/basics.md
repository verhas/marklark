---
title: Greeter guide
audience: developers
---
# Greeter guide

<!--CODE
emit("*Audience: " + front_matter()["audience"] + "*")
-->
<!--/CODE-->

## Contents

<!--CODE
tasks(depends_on=["sections"])
for h in headings(min_level=2, max_level=2):
    if h.text != "Contents":
        emit("- [" + h.text + "](#" + h.anchor + ")\n")
-->
<!--/CODE-->

<!--CODE
conf = toml("data/app.toml")
define("app", conf["app"])
define("port", conf["server"]["port"])
-->

This guide covers <!--= app.name-->?<!--/=--> version <!--$app.version-->? which listens on port
<!--$port-->? by default.

## The greet function

<!--CODE
emit(fence(slice("src/greet.py", start="# START greet", end="# END greet"), "python"))
-->
<!--/CODE-->

## Configuration

<!--CODE
s = json("data/settings.json")
emit("| Setting | Value |\n|---|---|\n")
emit("| Database | " + s["database"]["host"] + ":" + str(s["database"]["port"]) + " |\n")
emit("| Features | " + ", ".join(s["features"]) + " |\n")
-->
<!--/CODE-->

<!--CODE
tasks(completes=["sections"])
section = slice("CHANGELOG.md", section="2.1.0")
emit("## What is new in " + var("app.version") + "\n\n")
emit(section.split("\n", 1)[1].strip())
-->
<!--/CODE-->
