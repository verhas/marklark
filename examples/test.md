<!--CODE
define("marklark.version", "0.1.0");
#@ generated: 0:blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262
-->
<!--/CODE-->

<!--CODE
v = var("marklark")
emit(v["version"])
define("marklark.version", "0-2-0", lenient=True)
v = var("marklark")
emit("\n")
emit(v["version"])
#@ generated: 12:blake3:b3708c16b549ba1fb34a2502225f9186498bc822a509beb19405bad90383c510
-->
0.1.0
0-2-0
<!--/CODE-->

<!--CODE
emit(var("marklark.version"))
#@ generated: 6:blake3:397bf0d90be0a4545e0b6e9a611d2cad31302589cda1984fab7c2004d9192e1d
-->
0-2-0
<!--/CODE-->
