# The design board, v3.8.0 against v3.8.1

`cargo run --release -p computer-use --example design_bench`, the same
benchmark on both releases (the v3.8.0 run used this file's benchmark on
v3.8.0's code). No desktop is needed; the fastest of five runs is shown.
Token figures are estimates: text at about 4 characters a token, a
picture at width × height / 750.

| | v3.8.0 | v3.8.1 | |
|---|---:|---:|---|
| A badge, built and fixed (10 calls): tokens | 10,994 | **3,750** | −66% |
| of them, pictures | 9,791 | **2,651** | −73% |
| time | 141 ms | **90 ms** | −36% |
| A page of 150 layers (3 calls): tokens | 7,371 | **4,619** | −37% |
| time | 93 ms | **38 ms** | −60% |

Where it comes from:

- **Only the part of the picture that changed.** After the first
  picture, a change sends the part whose pixels changed and where it is
  (recolouring a star: 130×125 px, 22 tokens, instead of 1024×768, 1,049).
- **Exporting shows nothing new**, so it sends no picture or listing.
- **Each shape's lines are worked out once**, not again for every box,
  check, step and picture.
- **Look-alike layers in a row are one record** in the listing.

## v3.8.0


### A badge, built and fixed (10 calls)

| step | ms | text tok | picture | picture tok | total tok |
|---|---:|---:|---|---:|---:|
| start + 8 layers | 39.2 | 177 | 1024×768 | 1049 | 1226 |
| recolour one layer | 11.0 | 39 | 1024×768 | 1049 | 1088 |
| mirror an ear | 11.1 | 59 | 1024×768 | 1049 | 1108 |
| fix the eyes' height | 10.7 | 40 | 1024×768 | 1049 | 1089 |
| add 3 dots | 11.7 | 84 | 1024×768 | 1049 | 1133 |
| space the dots | 12.0 | 45 | 1024×768 | 1049 | 1094 |
| retitle | 11.4 | 45 | 1024×768 | 1049 | 1094 |
| look again | 12.9 | 227 | 1024×768 | 1049 | 1276 |
| zoom into a cell | 7.8 | 225 | 512×512 | 350 | 575 |
| export svg | 13.7 | 262 | 1024×768 | 1049 | 1311 |
| **total** | **141.4** | **1203** | | **9791** | **10994** |

### A page of 150 layers

| step | ms | text tok | picture | picture tok | total tok |
|---|---:|---:|---|---:|---:|
| 150 layers | 32.7 | 2089 | 1024×768 | 1049 | 3138 |
| recolour one of 150 | 28.5 | 46 | 1024×768 | 1049 | 1095 |
| look at 150 | 31.9 | 2089 | 1024×768 | 1049 | 3138 |
| **total** | **93.1** | **4224** | | **3147** | **7371** |

## v3.8.1


### A badge, built and fixed (10 calls)

| step | ms | text tok | picture | picture tok | total tok |
|---|---:|---:|---|---:|---:|
| start + 8 layers | 33.6 | 177 | 1024×768 | 1049 | 1226 |
| recolour one layer | 6.9 | 80 | 130×125 | 22 | 102 |
| mirror an ear | 6.0 | 74 | 124×147 | 25 | 99 |
| fix the eyes' height | 6.2 | 56 | 104×122 | 17 | 73 |
| add 3 dots | 6.3 | 100 | 310×96 | 40 | 140 |
| space the dots | 6.7 | 60 | 96×96 | 13 | 73 |
| retitle | 6.8 | 61 | 669×96 | 86 | 147 |
| look again | 8.7 | 214 | 1024×768 | 1049 | 1263 |
| zoom into a cell | 2.8 | 213 | 512×512 | 350 | 563 |
| export svg | 6.4 | 64 | — | 0 | 64 |
| **total** | **90.2** | **1099** | | **2651** | **3750** |

### A page of 150 layers

| step | ms | text tok | picture | picture tok | total tok |
|---|---:|---:|---|---:|---:|
| 150 layers | 16.3 | 1216 | 1024×768 | 1049 | 2265 |
| recolour one of 150 | 10.0 | 61 | 96×96 | 13 | 74 |
| look at 150 | 11.4 | 1231 | 1024×768 | 1049 | 2280 |
| **total** | **37.7** | **2508** | | **2111** | **4619** |
