# Panduan Pengembangan KolArk

Instruksi ini berlaku untuk seluruh workspace. Sebelum mengubah kode, baca `spesifikasi.md` dan aturan lokal yang berlaku pada direktori terkait. `spesifikasi.md` adalah sumber normatif untuk format arsip `.kola`; file ini mengatur cara mengimplementasikan, menguji, dan merilis produk tanpa menggantikan spesifikasi tersebut.

## Prioritas dan Prinsip

- Pertahankan format biner v1 persis seperti yang didefinisikan di `spesifikasi.md`. Jika instruksi implementasi tampak bertentangan dengan spesifikasi format, hentikan perubahan format dan selesaikan konflik melalui revisi terkontrol pada spesifikasi serta fixture.
- Gunakan istilah normatif MUST, MUST NOT, SHOULD, SHOULD NOT, dan MAY sesuai arti dalam spesifikasi.
- Utamakan determinisme, validasi ketat, perilaku gagal-tertutup, portabilitas, dan keselamatan data pengguna.
- Jangan mengklaim konformansi v1 atau Production Ready sebelum gerbang konformansi dan Definition of Done spesifikasi terpenuhi.
- Implementasikan hanya fitur yang benar-benar tersedia. Jangan menambah ekstensi privat, mengubah makna flags/reserved, atau mengklaim enkripsi, autentikasi penerbit, maupun kompatibilitas ZIP/TAR.
- Jaga perubahan tetap terfokus. Pertahankan API publik dan gaya kode yang ada kecuali tugas secara eksplisit memerlukan perubahan.

## Alur Kerja Perubahan

1. Periksa struktur proyek, instruksi yang berlaku, tes terkait, dan status perubahan yang sudah ada sebelum mengedit. Jangan menghapus atau menimpa perubahan pengguna.
2. Tentukan aturan spesifikasi yang mengendalikan perilaku yang diubah. Untuk perubahan format, sebutkan bagian spesifikasi dan dampak kompatibilitasnya dalam deskripsi perubahan.
3. Buat perubahan terkecil yang menyelesaikan kebutuhan. Jangan memasukkan refactor atau dependensi yang tidak diperlukan.
4. Tambahkan atau perbarui tes pada lapisan terdekat: unit, golden/conformance, round-trip, interoperabilitas, negative/security, atau property test sesuai risikonya.
5. Jalankan pemeriksaan terfokus terlebih dahulu, lalu gerbang proyek yang relevan (format, lint, test, build, audit). Laporkan dengan jelas setiap pemeriksaan yang tidak dapat dijalankan.
6. Untuk perubahan berpengaruh pada byte, uji kompatibilitas silang dan hasil byte-for-byte; jangan memperbarui golden vector hanya agar implementasi tampak lulus.

## Kontrak Format v1

- Arsip memakai magic `KOLA`, versi biner tepat `1`, little-endian untuk integer multibyte, dan footer tetap 72 byte. Reader v1 menolak versi atau flags yang tidak didukung.
- Gunakan ukuran field, tata letak, offset, urutan, encoding, dan aturan canonical persis seperti `spesifikasi.md`. Semua operasi `offset + length`, perkalian jumlah elemen, delta timestamp, dan ukuran output wajib memakai pemeriksaan overflow.
- Implementasikan varint unsigned LEB128 dan signed Zigzag secara canonical; tolak encoding nonminimal, terpotong, overflow, atau melebihi batas field.
- BLAKE3-256 menggunakan unkeyed mode tanpa salt/prefix. Hash chunk dihitung atas byte asli; hash file atas seluruh isi; `hash_all` atas byte arsip sebelum footer. Jangan menyebut hash ini sebagai autentikasi penerbit.
- Gear CDC harus memakai tepat 256 nilai hasil SplitMix64 Lampiran A, state `u32` wrap modulo `2^32`, rumus boundary dan reset yang ditetapkan, serta pemotongan wajib pada ukuran maksimum.
- KolArk-LZ adalah codec format sendiri, bukan pengganti LZ4/zlib/gzip. Decoder harus menerapkan seluruh validasi stream dan batas output. Encoder harus mengikuti aturan greedy canonical termasuk tie-break offset terkecil.
- Simpan chunk raw bila stream LZ tidak lebih kecil. Pilihan codec kolom harus mengikuti aturan canonical spesifikasi.
- Pertahankan deduplikasi berdasarkan hash **dan** perbandingan ukuran/byte. Tangani collision hash sebagai entri terpisah. Indeks chunk diurutkan menurut hash leksikografis, lalu offset menaik; referensi metadata menunjuk indeks final 0-based.
- Writer harus mengurutkan path berdasarkan byte UTF-8, dictionary menurut byte UTF-8, metadata dalam enam kolom berurutan, dan chunk index secara canonical. Urutan penemuan chunk fisik mengikuti file canonical dan byte file.
- Jangan mengubah magic, versi, tata letak, Gear, CDC, BLAKE3, KolArk-LZ, metadata, deduplikasi, urutan canonical, atau aturan path secara diam-diam. Perubahan tersebut memerlukan revisi spesifikasi dan fixture serta keputusan versi yang eksplisit.

## Arsitektur dan Bahasa

- Ikuti struktur dan bahasa yang sudah ada. Bila membangun implementasi referensi baru, spesifikasi merekomendasikan Rust stable dengan edition/toolchain dipatok, pemisahan tanggung jawab format, codec, core reader/writer, ekstraksi, dan CLI, serta Python sebagai implementasi pembanding interoperabilitas bila tersedia.
- Jangan memaksakan struktur crate yang direkomendasikan sebelum kebutuhan repositori menuntutnya. Hindari circular dependency dan pertahankan batas antara format, codec, I/O, ekstraksi, dan antarmuka pengguna.
- Hindari `unsafe`; jika benar-benar diperlukan, berikan alasan terukur, tinjauan keselamatan, dan tes khusus. Jangan memakai `unwrap()` atau `expect()` pada jalur yang memproses arsip tidak tepercaya.
- Parser harus bounded dan streaming bila memungkinkan. Jangan mengalokasikan langsung berdasarkan panjang dari arsip sebelum memvalidasi rentang, overflow, serta limit operasional.
- Gunakan dependency aktif, kompatibel lisensi, dan minimum yang diperlukan. Catat dependency di lockfile; jangan mengunduh dependency saat runtime. Dependency codec eksternal tidak boleh menggantikan KolArk-LZ.
- Jangan menambahkan stub, placeholder, test-only path hardcoded, atau jalur yang melaporkan sukses tanpa melakukan operasi yang dijanjikan.

## Keamanan dan Ketahanan

- Perlakukan setiap arsip sebagai input tidak tepercaya dan lakukan fail closed untuk header, flags, offset, ukuran, varint, codec, metadata, indeks, hash, path, dan referensi yang tidak valid.
- Verifikasi `hash_all` sebelum ekstraksi; verifikasi hash chunk sesudah dekompresi dan hash file sesudah chunk digabung. Cocokkan ukuran hasil pada setiap lapisan.
- Terapkan limit konfigurasi untuk jumlah file/chunk, memori, ukuran kolom, byte output, dan kerja/dekompresi. Limit implementasi yang lebih rendah dari batas format harus menghasilkan error resource-limit yang jelas, bukan membuat parser menerima data tak terbatas.
- Path harus UTF-8 valid, relatif, memakai `/`, maksimal 4096 byte, serta tanpa path absolut, komponen kosong/`.`/`..`, NUL, backslash, prefix drive, atau skema absolut. Tolak path duplikat dan tangani collision pada filesystem case-insensitive tanpa overwrite diam-diam.
- Pastikan semua hasil ekstraksi tetap di dalam direktori target. Jangan mengandalkan pemeriksaan string path saja; cegah symlink traversal dengan operasi filesystem aman terhadap symlink atau mekanisme ekuivalen.
- Jangan overwrite tanpa kebijakan eksplisit. Tulis ke file sementara dan rename atomik. Jangan tinggalkan hasil parsial yang dilaporkan sukses saat validasi atau I/O gagal.
- V1 hanya menyimpan file reguler. Tolak mode tipe file lain. Bersihkan bit SUID/SGID/sticky secara default dan jangan menerapkan permission lebih permisif dari kebijakan pengguna.
- Hardlink/reflink saat ekstraksi bersifat opsional. Jika metadata inode tidak kompatibel atau operasi gagal, salin secara normal; jangan mengubah metadata bersama yang memengaruhi path lain.

## Produk dan CLI

- Jika CLI diterapkan, pertahankan subcommand kontrak produk: `create`, `list`, `extract`, `verify`, `info`, `test-vector`, dan `version`. Opsi yang tidak didukung, termasuk `--follow-symlinks` pada v1, harus ditolak dengan jelas.
- Jaga exit code stabil: `0` sukses, `1` umum, `2` argumen, `3` arsip/format rusak, `4` versi/fitur tak didukung, `5` I/O, `6` limit sumber daya, `7` integritas/hash, `8` konflik tujuan/overwrite.
- Error manusia ditulis ke stderr; data dan JSON ke stdout. Jangan mengubah output CLI yang sudah menjadi kontrak tanpa tes dan pembaruan dokumentasi.
- Operasi stdin/stdout hanya boleh didukung bila batasan seek/footer ditangani dengan spooling atau penolakan yang jelas. Writer non-seekable tidak boleh menghasilkan arsip tak valid.
- `dry-run` tetap harus memvalidasi tingkat struktur/integritas yang dijanjikan. Opsi ekstraksi seperti strip-components tidak boleh meniadakan validasi keamanan path.

## Persyaratan Pengujian

- Pertahankan unit test untuk varint/ZigZag, BLAKE3 known-answer, Gear table/CDC, KolArk-LZ, metadata, indeks, serta path.
- Uji input kosong, file kosong, satu dan banyak file, multi-chunk, deduplikasi, collision hash simulasi, chunk/kolom raw dan LZ, serta parameter batas.
- Uji penolakan magic/versi/flags salah, varint invalid, data terpotong, ukuran/offset overflow, overlap, indeks salah urut, hash mismatch, codec invalid, dan referensi di luar rentang.
- Uji traversal, path absolut, komponen tidak valid, NUL/backslash, collision nama, symlink escape, overwrite, serta resource exhaustion.
- Golden vector wajib diverifikasi independen: BLAKE3 KAT, parser kedua untuk offset/ukuran, round-trip byte identik, cross-read Rust/Python, cross-write byte-identik, dan manifest SHA-256. Catat input serta versi generator.
- Jangan memperlakukan angka fixture pada spesifikasi sebagai bukti mandiri; ikuti proses verifikasi §27.2 sebelum membekukan atau memperbaruinya.
- Crash fuzz harus direproduksi sebagai regression test. Fuzz target tidak boleh menulis di luar direktori sementara. Jalankan pemeriksaan sanitizer/Miri bila modul dan toolchain mendukungnya.
- Optimasi dan paralelisme boleh diterapkan hanya jika output canonical dan validasi tetap sama. Ukur benchmark; jangan mengurangi pemeriksaan keamanan demi kinerja.

## CI, Dokumentasi, dan Rilis

- Untuk perubahan kode, jalankan gerbang yang tersedia: format check, lint, unit/integration, golden/conformance, build CLI, dan audit dependency. Perubahan parser/ekstraksi memerlukan tes negative/security yang relevan.
- CI pull request harus mencakup format, lint, tes, conformance, audit dependency, build target utama, dan kompilasi dokumentasi/contoh yang tersedia.
- Rilis menambahkan build lintas platform yang didukung, interoperabilitas, fuzz regression, benchmark, SBOM, checksum artifact, provenance/signature bila tersedia, dan verifikasi reproducible build.
- Pisahkan format version, revisi spesifikasi, dan versi aplikasi. Pembaruan editorial tidak boleh mengubah byte format; perubahan interpretasi byte memerlukan proses versi format.
- Perbarui dokumentasi pengguna, API/CLI, security guide, compatibility matrix, dan changelog bila perilaku terkait berubah. Jangan mendokumentasikan fitur yang belum ada.
- Klaim `KolArk v1 conforming` hanya setelah suite konformansi lulus. Klaim `Production Ready` hanya setelah semua gerbang Definition of Done §31 terbukti.

## Pelaporan Selesai

Saat menyelesaikan perubahan, rangkum bagian yang berubah, aturan/risiko format yang terpengaruh, tes dan perintah validasi yang lulus, serta pemeriksaan yang tidak dapat dijalankan. Nyatakan secara eksplisit bila perubahan belum membuktikan konformansi atau kesiapan produksi.