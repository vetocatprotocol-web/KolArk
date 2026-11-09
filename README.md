# KolArk

**Columnar Archive** adalah rancangan format arsip terbuka yang menggabungkan content-defined chunking (CDC), deduplikasi, kompresi blok, metadata kolumnar, dan verifikasi integritas.

> **Status: dalam pengembangan.** Workspace kini memiliki implementasi Rust untuk format, codec, writer/reader arsip, ekstraksi dasar, dan CLI. Suite konformansi independen, interoperabilitas Rust/Python, serta beberapa jaminan keselamatan filesystem belum lengkap. Belum ada rilis yang dinyatakan conforming atau production-ready.

KolArk tidak membungkus atau mengklaim kompatibilitas dengan ZIP maupun TAR. Format arsipnya sendiri menggunakan ekstensi `.kola`, magic `KOLA`, dan versi biner `1`.

## Cara Kerja

KolArk dirancang untuk memecah isi file menjadi chunk berbasis Gear Hash CDC, menyimpan setiap chunk unik satu kali, lalu mengompresnya menggunakan codec KolArk-LZ. Metadata file disimpan dalam enam kolom, sedangkan indeks chunk dan footer berada di akhir arsip agar reader dapat menemukan struktur utama dari belakang file.

Format v1 menetapkan output canonical: input dan opsi yang sama harus menghasilkan arsip identik pada implementasi yang conforming.

| Komponen | Rancangan v1 |
|---|---|
| Hash | BLAKE3-256 unkeyed untuk chunk, isi file, dan body arsip |
| Chunking | Gear Hash CDC dengan tabel 256 nilai deterministik |
| Deduplikasi | Berdasarkan hash dan perbandingan byte untuk menangani collision |
| Kompresi | KolArk-LZ per chunk dan payload kolom |
| Metadata | Enam kolom: path, timestamp, ukuran, mode, referensi chunk, dan hash isi |
| Ekstraksi | File reguler dengan validasi path dan batas sumber daya |

## Status Implementasi

Workspace Rust memiliki lima crate: `kolark-format`, `kolark-codec`, `kolark-core`, `kolark-extract`, dan `kolark-cli`. Implementasi mencakup serialisasi format, varint/Zigzag, BLAKE3, Gear CDC, KolArk-LZ, penulisan/pembacaan arsip, verifikasi, serta ekstraksi dan perintah CLI utama.

CLI dapat dibangun dan digunakan untuk pekerjaan dasar, tetapi bukan pengganti suite konformansi. `test-vector` menjalankan pemeriksaan codec bawaan; fixture arsip konformansi tidak dibundel. Ekstraksi juga belum menjamin perlindungan race filesystem tingkat produksi. Status KolArk v1 conforming dan kesiapan produksi belum terbukti.

## Cakupan Format v1

V1 dirancang untuk:

- Mengarsipkan file reguler dengan path relatif UTF-8.
- Membagi file menjadi chunk menggunakan Gear Hash CDC dan menduplikasi konten yang identik.
- Mengompres chunk dan kolom metadata menggunakan KolArk-LZ sesuai aturan canonical.
- Memvalidasi struktur, offset, ukuran, referensi, dan hash sebelum atau selama pembacaan.
- Mengekstrak file dengan validasi path dan pemeriksaan symlink; perlindungan race filesystem masih memerlukan penyelesaian.

V1 tidak menyediakan enkripsi, tanda tangan digital, autentikasi penerbit, symbolic link, hardlink sebagai entri, special file, ACL, extended attributes, atau metadata owner/group. Hash tanpa kunci mendeteksi perubahan data, tetapi **tidak** membuktikan siapa pembuat arsip. Arsip yang diubah beserta hash-nya dapat dihitung ulang oleh pihak lain.

## Rancangan CLI

Nama executable yang direncanakan adalah `kolark`. Kontrak produk mendefinisikan perintah berikut setelah implementasi tersedia:

| Perintah | Tujuan |
|---|---|
| `kolark create <archive.kola> <paths...>` | Membuat arsip dari file reguler |
| `kolark list <archive.kola>` | Menampilkan isi arsip |
| `kolark extract <archive.kola> [-C DIR]` | Mengekstrak ke direktori target |
| `kolark verify <archive.kola>` | Memvalidasi struktur dan hash arsip |
| `kolark info <archive.kola>` | Menampilkan informasi arsip |
| `kolark test-vector` | Menjalankan pemeriksaan codec bawaan (bukan fixture konformansi penuh) |
| `kolark version` | Menampilkan versi aplikasi dan format |

Perintah tersedia melalui `cargo run -p kolark-cli -- <perintah>`. Opsi `--json` menghasilkan JSON untuk perintah yang mendukung output; `--quiet` menekan output sukses teks. Opsi yang tidak didukung, termasuk `--follow-symlinks`, ditolak oleh parser CLI.

## Tata Letak Workspace

```text
.
├── Cargo.toml
├── AGENTS.md
├── spesifikasi.md
├── spesifikasi_SHA256.txt
└── crates/
    ├── kolark-format/
    ├── kolark-codec/
    ├── kolark-core/
    ├── kolark-extract/
    └── kolark-cli/
```

Spesifikasi produk merekomendasikan pemisahan tanggung jawab sebagai berikut:

- `kolark-format`: tipe dan serialisasi struktur biner.
- `kolark-codec`: BLAKE3, Gear CDC, KolArk-LZ, dan varint.
- `kolark-core`: reader, writer, metadata, serta indeks.
- `kolark-extract`: ekstraksi yang aman.
- `kolark-cli`: antarmuka command line `kolark`.

Lihat [spesifikasi format dan produk](spesifikasi.md) untuk definisi lengkap dan normatif. [AGENTS.md](AGENTS.md) memuat panduan kontribusi dan pengembangan untuk workspace.

## Pengembangan

Implementasi dirancang menggunakan Rust stable, edition 2021. Setelah workspace dan toolchain tersedia, pemeriksaan dasar yang ditargetkan adalah:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

Untuk perubahan parser, codec, indeks, atau ekstraksi, tambahkan tes negatif dan keamanan yang sesuai. Perubahan yang memengaruhi byte arsip juga memerlukan golden vector independen serta pengujian interoperabilitas. Lulusnya tes workspace saja tidak membuktikan konformansi.

## Benchmark

Benchmark aplikasi dapat dijalankan dalam profile release:

```sh
cargo run --release -p kolark-bench -- --iterations 3 --size-mib 4
```

Harness mengeluarkan laporan JSON untuk empat workload sintetis: banyak file kecil, file besar dengan data pseudo-acak, file besar berulang, dan beberapa file besar identik. Setiap workload mengukur pembuatan arsip, open/list, verifikasi penuh, ekstraksi, rasio ukuran arsip terhadap input, rasio deduplikasi, serta process peak memory bila tersedia. Dataset disiapkan di luar timer; waktu mencakup operasi arsip terkait. Iterasi dilaporkan satu per satu dan throughput memakai median waktu. `--size-mib` mengatur ukuran satu file besar; workload file duplikat berisi empat salinan file berukuran tersebut.

`process_peak_memory_mib` adalah high-water mark seluruh proses worker (termasuk dataset dan arsip yang ditahan untuk pengukuran) dan saat ini hanya tersedia pada Linux melalui `VmHWM`. Benchmark ini sintetis, hasilnya bergantung pada mesin dan beban sistem, serta tidak membuktikan target kinerja, konformansi, atau kesiapan produksi.

## Continuous Integration

Workflow GitHub Actions di `.github/workflows/ci.yml` berjalan pada push, pull request, dan pemicu manual. Format check, Clippy, seluruh unit/integration tests, build release CLI, dan kompilasi dokumentasi Rust dijalankan pada Linux, macOS, dan Windows. Dependency audit berjalan terpisah, sementara benchmark smoke test dijalankan sekali pada Linux dan laporan JSON disimpan sebagai artifact.

Workflow ini belum menjalankan golden/conformance vectors, interop Rust/Python, atau fuzz smoke tests karena fixture lintas implementasi dan harness fuzz belum tersedia. Kelulusan CI tidak menyatakan format v1 conforming atau produk siap produksi.

## Konformansi dan Rilis

Implementasi tidak boleh menyatakan **KolArk v1 conforming** sebelum lulus suite konformansi, termasuk known-answer tests BLAKE3, pengujian Gear CDC dan KolArk-LZ, golden vectors, validasi arsip rusak, serta cross-read dan byte-identical cross-write Rust/Python.

Label **Production Ready** memerlukan seluruh gerbang Definition of Done dalam spesifikasi: reader dan writer yang tervalidasi, ekstraksi aman, batas sumber daya, fuzzing, interoperabilitas, CI lintas target, dokumentasi, dan proses distribusi yang dapat diverifikasi.

## Lisensi

Manifest workspace saat ini menyatakan `MIT OR Apache-2.0`. Periksa berkas lisensi proyek dan distribusi dependency sebelum menggunakan atau mendistribusikan kode.