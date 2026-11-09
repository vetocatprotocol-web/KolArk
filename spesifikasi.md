# KolArk (Columnar Archive)
## Spesifikasi Format Arsip Biner — Versi 1.0.0

**Revisi dokumen:** 1.0.1  
**Versi biner:** `1`  
**Ekstensi:** `.kola`  
**Magic:** `KOLA`  
**Status:** Spesifikasi normatif untuk implementasi dan uji interoperabilitas  
**Bahasa:** Indonesia

> **Status kompatibilitas.** Dokumen ini membakukan rancangan yang diberikan dan memperbaiki bagian yang sebelumnya tidak deterministik atau tidak lengkap. Arsip yang mengikuti draf terdahulu tidak otomatis kompatibel. Perubahan penting pada revisi ini: footer 72 byte ditegaskan, tabel Gear Hash dibuat deterministik tepat 256 entri, field codec kolom ditambahkan, serta aturan encoder/decoder KolArk-LZ, urutan indeks, dan validasi arsip ditetapkan. Nomor versi biner tetap `1` untuk spesifikasi yang dibakukan di dokumen ini; implementasi eksperimental dari draf sebelum pembakuan tidak boleh mengklaim konformansi v1.

---

## Daftar Isi

1. Ringkasan dan ruang lingkup  
2. Bahasa normatif dan konvensi  
3. Tipe data dasar  
4. Identitas format dan kompatibilitas  
5. Tata letak file  
6. Header  
7. Superblock  
8. Chunk Store  
9. Content-Defined Chunking (CDC)  
10. Deduplikasi dan Chunk Index  
11. Table Metadata  
12. Kompresi KolArk-LZ  
13. Footer dan prosedur pembacaan  
14. Aturan penulisan arsip kanonis  
15. Prosedur pembacaan dan ekstraksi  
16. Integritas, keamanan, dan batas sumber daya  
17. Batasan versi 1  
18. Interoperabilitas dan konformansi  
19. Test vector  
20. Ringkasan struktur biner  
Lampiran A. Algoritma pembangkitan tabel Gear  
Lampiran B. Catatan keputusan pembakuan

---

# 1. Ringkasan dan Ruang Lingkup

**KolArk** (*Columnar Archive*) adalah format arsip biner yang menggabungkan tiga prinsip utama:

1. **Deduplikasi konten** berbasis *Content-Defined Chunking* (CDC) dan hash BLAKE3-256.
2. **Metadata kolumnar**, sehingga atribut file disimpan dalam enam kolom terpisah dan dapat dikodekan serta dikompresi per kolom.
3. **Indeks di ekor file**, sehingga pembaca dapat menemukan metadata dan indeks dengan membaca footer di akhir arsip terlebih dahulu.

Tujuannya adalah menghasilkan satu arsip yang hemat ruang, dapat diperiksa integritasnya, mendukung akses indeks tanpa memindai seluruh arsip, dan dapat diekstrak dengan deduplikasi otomatis.

## 1.1 Cakupan KolArk v1

Versi biner 1 mendukung:

- file reguler (*regular file*);
- path relatif berformat UTF-8;
- timestamp modifikasi dalam detik Unix;
- mode POSIX 16-bit;
- pemotongan konten menjadi chunk dengan Gear Hash CDC;
- hash BLAKE3-256 untuk chunk, file, dan body arsip;
- deduplikasi chunk;
- kompresi KolArk-LZ untuk chunk dan payload kolom metadata;
- metadata kolumnar enam kolom;
- ekstraksi aman ke direktori target.

## 1.2 Di luar cakupan v1

Versi 1 tidak menyimpan atau memulihkan symbolic link, direktori kosong sebagai entri mandiri, hardlink sumber sebagai tipe entri, device node, socket, FIFO, owner/group ID, ACL, extended attributes, alternate data streams, resource fork, nanodetik timestamp, enkripsi, atau tanda tangan digital. Direktori induk dibuat secara otomatis saat ekstraksi.

BLAKE3 yang digunakan di sini adalah hash tanpa kunci (*unkeyed hash*). Hash memberi deteksi perubahan data, bukan autentikasi penerbit arsip. Pihak yang mengubah arsip dan menghitung ulang footer juga dapat menghasilkan arsip dengan hash yang valid.

---

# 2. Bahasa Normatif dan Konvensi

Istilah **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, **SHOULD NOT**, dan **MAY** memiliki arti normatif:

- **MUST / MUST NOT**: wajib / dilarang.
- **SHOULD / SHOULD NOT**: rekomendasi kuat; penyimpangan memerlukan alasan teknis.
- **MAY**: pilihan yang diizinkan.

Konvensi biner:

| Konvensi | Ketentuan |
|---|---|
| Endianness | Little-endian untuk semua integer multibyte |
| Byte string | Byte mentah tanpa NUL terminator, kecuali dinyatakan |
| String | Panjang byte sebagai `varint`, diikuti byte UTF-8 |
| Offset | `u64` dihitung dari byte pertama arsip (offset 0) |
| Ukuran | Byte; field menentukan apakah `u32` atau `u64` |
| Padding | Tidak ada padding antarbagian atau antarrecord |
| Reserved | Penulis mengisi nol; pembaca mengabaikan kecuali aturan field menyatakan harus nol |
| Path separator | `/` (slash) |
| Urutan canonical | Mengikuti aturan eksplisit tiap bagian; tidak bergantung urutan enumerasi filesystem |

Semua penjumlahan offset dan ukuran MUST diperiksa terhadap overflow sebelum alokasi, pembacaan, atau pencarian posisi file.

---

# 3. Tipe Data Dasar

| Tipe | Ukuran | Definisi |
|---|---:|---|
| `u8` | 1 byte | Integer unsigned 8-bit |
| `u16` | 2 byte | Integer unsigned 16-bit, LE |
| `u32` | 4 byte | Integer unsigned 32-bit, LE |
| `u64` | 8 byte | Integer unsigned 64-bit, LE |
| `i64` | 8 byte | Integer signed 64-bit, LE |
| `varint` | 1–10 byte | Unsigned LEB128 untuk `u64` |
| `svarint` | 1–10 byte | Zigzag `i64`, kemudian unsigned LEB128 |
| `hash256` | 32 byte | Digest BLAKE3-256 |
| `bytes(n)` | n byte | Byte mentah sepanjang n byte |
| `string` | variabel | `varint(byte_length) + UTF-8 bytes` |

## 3.1 Unsigned LEB128

Setiap byte menyimpan tujuh bit data. Bit paling tinggi (MSB) adalah continuation bit: `1` berarti byte berikutnya masih bagian dari nilai; `0` berarti nilai berakhir.

Contoh:

```text
300 = 0xAC 0x02
0xAC: tujuh bit data 0x2C, continuation = 1
0x02: tujuh bit data 0x02, continuation = 0
300 = 0x2C + (0x02 << 7)
```

Aturan decoder:

1. Nilai harus berada dalam rentang `0..=2^64-1`.
2. Panjang maksimum adalah 10 byte.
3. Pada byte ke-10, hanya bit data paling rendah yang boleh bernilai satu; bit data lain dan continuation bit MUST nol.
4. Encoding nonminimal MUST ditolak. Contoh: nilai 1 tidak boleh dikodekan sebagai `81 00`.
5. Varint terpotong, overflow, atau melebihi batas field MUST ditolak.

## 3.2 Signed varint (Zigzag)

Konversi dilakukan sebelum encoding LEB128:

```text
encode(n: i64) = ((n << 1) XOR (n >> 63)) interpreted as u64
decode(u: u64) = (u >> 1) XOR -(u AND 1)
```

Implementasi MUST menghasilkan pemetaan yang sama untuk seluruh rentang `i64`, termasuk `i64::MIN` dan `i64::MAX`. Implementasi bahasa yang tidak mendefinisikan shift signed harus memakai operasi ekuivalen yang teruji.

## 3.3 String UTF-8

String diawali panjang byte sebagai varint dan diikuti byte UTF-8 tanpa terminator. Decoder MUST menolak UTF-8 yang tidak valid. Path dibatasi maksimum 4096 byte UTF-8 pada v1. String non-path mengikuti batas struktur yang memuatnya.

## 3.4 BLAKE3-256

Semua hash memakai BLAKE3 mode hash biasa (*unkeyed mode*) dengan output tepat 32 byte, tanpa truncation, salt, key, atau prefix domain:

- `chunk_hash = BLAKE3(raw_chunk_bytes)`.
- `content_hash = BLAKE3(seluruh byte file asli)`.
- `hash_all = BLAKE3(seluruh byte arsip sebelum footer)`.

Hash chunk selalu dihitung sebelum kompresi. Hash file dihitung secara streaming dan tidak bergantung pada batas chunk.

---

# 4. Identitas Format dan Kompatibilitas

- Magic arsip: empat byte `b"KOLA"` (`4B 4F 4C 41`).
- Versi biner v1: `u32 = 1`.
- Pembaca v1 MUST menolak `version != 1`; menerima versi `0` atau versi yang tidak dikenal tidak diperbolehkan.
- Perubahan struktur biner yang mengubah interpretasi field memerlukan versi biner baru.
- Perubahan implementasi yang tidak mengubah byte format tidak memerlukan kenaikan versi arsip.

Versi biner tidak sama dengan revisi dokumen atau versi aplikasi.

---

# 5. Tata Letak File

Arsip tersusun dalam urutan berikut:

```text
Offset 0
+----------------------------------------+
| HEADER                         16 byte |
+----------------------------------------+
| SUPERBLOCK                     32 byte |
+----------------------------------------+
| CHUNK STORE                              |
| [chunk record 0][chunk record 1]...      |
+----------------------------------------+
| TABLE METADATA                           |
| [column 0][column 1]...[column 5]        |
+----------------------------------------+
| CHUNK INDEX                              |
| count + sorted entries                   |
+----------------------------------------+
| FOOTER                         72 byte |
+----------------------------------------+
END OF FILE
```

Offset tetap dan dinamis:

| Bagian | Offset |
|---|---:|
| Header | `0` |
| Superblock | `16` |
| Awal Chunk Store | `48` |
| Table Metadata | `offset_table_meta` di footer |
| Chunk Index | `offset_chunk_index` di footer |
| Footer | `file_size - 72` |

Chunk Store boleh kosong. Table Metadata tetap selalu berisi tepat enam record kolom. Chunk Index selalu memuat field `count`, termasuk jika nilainya nol. Tidak ada padding di antara bagian.

Syarat urutan offset:

```text
48 <= offset_table_meta < offset_chunk_index < footer_start
footer_start = file_size - 72
```

`offset_superblock` harus `16`, `offset_chunk_store` harus `48`. Semua offset harus berada dalam batas file dan tidak boleh tumpang tindih.

---

# 6. Header (16 Byte)

```rust
struct Header {
    magic:     [u8; 4],  // b"KOLA"
    version:   u32,      // 1
    flags:     u32,      // wajib 0 pada v1
    reserved:  u32,      // ditulis 0
}
```

| Offset relatif | Ukuran | Field |
|---:|---:|---|
| 0 | 4 | `magic` |
| 4 | 4 | `version` |
| 8 | 4 | `flags` |
| 12 | 4 | `reserved` |

Penulis MUST menulis `magic=KOLA`, `version=1`, `flags=0`, dan `reserved=0`. Pembaca MUST menolak magic, versi, atau flags yang tidak didukung. Nilai `reserved` boleh diabaikan pembaca.

---

# 7. Superblock (32 Byte)

Superblock menetapkan parameter CDC dan algoritma dasar arsip.

```rust
struct Superblock {
    chunk_size_min: u32,       // 4 byte
    chunk_size_avg: u32,       // 4 byte
    chunk_size_max: u32,       // 4 byte
    hash_alg:       u8,        // 0 = BLAKE3-256
    compression:    u8,        // 0 = KolArk-LZ untuk chunk
    flags:          u16,       // wajib 0 pada v1
    reserved:       [u8; 16],  // ditulis nol
}
```

Total: `4 + 4 + 4 + 1 + 1 + 2 + 16 = 32 byte`.

Nilai rekomendasi:

| Field | Nilai | Arti |
|---|---:|---|
| `chunk_size_min` | 16384 | 16 KiB |
| `chunk_size_avg` | 65536 | Target mean chunk sekitar 64 KiB |
| `chunk_size_max` | 262144 | 256 KiB |
| `hash_alg` | 0 | BLAKE3-256 |
| `compression` | 0 | KolArk-LZ |
| `flags` | 0 | Tidak ada flag v1 |

Validasi parameter CDC:

- `chunk_size_min >= 1`.
- `chunk_size_min < chunk_size_avg < chunk_size_max`.
- `hash_alg` MUST sama dengan `0`.
- `compression` MUST sama dengan `0`.
- `flags` MUST sama dengan `0`.
- `reserved` ditulis nol dan diabaikan saat dibaca.

`chunk_size_avg` adalah target rata-rata statistik, bukan jaminan setiap chunk memiliki ukuran tepat nilai tersebut. Batas maksimum selalu ditegakkan.

---

# 8. Chunk Store

Chunk Store berisi chunk dari byte file asli. Setiap chunk disimpan sebagai satu record berurutan. Record duplikat tidak disimpan ulang jika hash dan byte aslinya identik.

## 8.1 Format Chunk Record

```rust
struct ChunkRecord {
    hash:            [u8; 32], // BLAKE3 dari data asli
    size_compressed: u32,      // jumlah byte payload tersimpan
    size_original:   u32,      // jumlah byte setelah dekompresi
    flags:           u8,       // bit 0 menentukan raw/LZ
    data:            [u8; size_compressed],
}
```

Header record berukuran tepat `32 + 4 + 4 + 1 = 41 byte`. Ukuran total record:

```text
chunk_record_size = 41 + size_compressed
```

## 8.2 Flag Chunk

| `flags` | Arti |
|---:|---|
| `0x00` | Payload dikodekan sebagai stream KolArk-LZ |
| `0x01` | Payload disimpan raw/tanpa kompresi |

Bit lain reserved dan MUST bernilai nol. Pembaca MUST menolak bit yang tidak dikenal.

Aturan konsistensi:

- Untuk `flags=0x00`, decoder LZ wajib menghasilkan tepat `size_original` byte.
- Untuk `flags=0x01`, `size_compressed` MUST sama dengan `size_original`.
- `size_original` MUST lebih besar dari nol untuk record chunk yang benar-benar direferensikan.
- `size_original` MUST tidak melebihi `chunk_size_max`.
- `size_compressed` dan seluruh rentang record MUST berada dalam batas Chunk Store.
- Hash BLAKE3 atas data hasil dekompresi MUST sama dengan field `hash`.

Penulis MUST menyimpan raw jika ukuran stream LZ lebih besar atau sama dengan ukuran asli. Keputusan ini menjadi aturan canonical pada §14.

## 8.3 Urutan Fisik

Chunk pertama dimulai tepat di offset `48`. Record selanjutnya dimulai tepat setelah byte terakhir record sebelumnya. Tidak ada celah, padding, atau alignment. Batas akhir Chunk Store sama dengan `offset_table_meta`.

## 8.4 File Kosong

File berukuran nol tidak menghasilkan chunk record. Nilai `chunk_refs` untuk file kosong memiliki `chunk_count=0`. `content_hash` file kosong adalah BLAKE3 atas input kosong.

---

# 9. Content-Defined Chunking (CDC)

KolArk v1 memakai Gear Hash dengan tabel 256 entri `u32`. Algoritma yang dipakai bukan rolling window dengan buffer byte; state Gear diperbarui untuk setiap byte yang dikonsumsi dan direset pada batas chunk.

## 9.1 Tabel Gear

Tabel MUST berisi tepat 256 nilai `u32`. Untuk menghilangkan ketergantungan pada tabel literal yang salah panjang atau berubah antarimplementasi, nilai tabel dibangkitkan dengan algoritma deterministik di Lampiran A. Seed dan seluruh konstanta pembangkit adalah bagian dari spesifikasi format dan tidak boleh diganti pada v1.

Tiap implementasi dapat membangkitkan tabel saat inisialisasi atau menyimpannya sebagai konstanta hasil pembangkitan, tetapi seluruh 256 nilai harus identik.

## 9.2 Parameter dan Boundary Probability

Definisikan:

```text
boundary_span = chunk_size_avg - chunk_size_min
```

Setelah minimum tercapai, boundary alami dipilih ketika:

```text
(state % boundary_span) == 0
```

Dengan asumsi state tersebar cukup merata, jarak ekspektasi setelah batas minimum mendekati `boundary_span`, sehingga mean chunk mendekati `chunk_size_avg`. Ini merupakan target statistik; batas maksimum dan distribusi input dapat mengubah rata-rata aktual.

## 9.3 Pembaruan State

State adalah integer unsigned 32-bit. Setelah membaca byte `b`:

```text
state = ((state << 1) + GEAR_TABLE_256[b]) mod 2^32
position = position + 1
```

Arithmetic overflow harus wrap modulo `2^32`, bukan panic dan bukan integer presisi tak terbatas.

## 9.4 Prosedur CDC

```text
state = 0
position = 0
chunk_buffer = empty

for each byte b in input:
    append b to chunk_buffer
    state = ((state << 1) + GEAR_TABLE_256[b]) mod 2^32
    position += 1

    if position == chunk_size_max:
        emit chunk_buffer
        clear chunk_buffer
        state = 0
        position = 0
    else if position >= chunk_size_min
         and (state % (chunk_size_avg - chunk_size_min)) == 0:
        emit chunk_buffer
        clear chunk_buffer
        state = 0
        position = 0

if chunk_buffer is not empty:
    emit chunk_buffer
```

Aturan tambahan:

- Pemotongan pada `chunk_size_max` bersifat wajib, meskipun boundary alami tidak terjadi.
- Boundary alami tidak boleh terjadi sebelum `chunk_size_min`.
- State dan position selalu direset setelah chunk dikeluarkan.
- Sisa input di akhir stream menjadi chunk terakhir, walaupun ukurannya di bawah minimum.
- Input kosong menghasilkan nol chunk.
- CDC berjalan pada byte file asli sebelum kompresi.

---

# 10. Deduplikasi dan Chunk Index

## 10.1 Deduplikasi

Penulis MUST menghitung BLAKE3 dari byte asli chunk sebelum kompresi. Ketika hash baru sudah ada di map internal, penulis MUST membandingkan ukuran dan byte asli untuk memastikan chunk benar-benar identik sebelum memakai record yang sudah ada.

- Hash sama dan byte sama: gunakan record yang sudah ada, jangan menulis record kedua.
- Hash sama tetapi byte berbeda: perlakukan sebagai hash collision; simpan sebagai record terpisah dan indekskan secara terpisah.
- Hash berbeda: simpan sebagai record baru.

Aturan collision membuat format tidak bergantung pada asumsi bahwa collision mustahil. Referensi metadata menunjuk indeks tertentu, bukan hanya hash.

## 10.2 Format Chunk Index

```rust
struct ChunkIndex {
    count: u32,
    entries: [ChunkIndexEntry; count],
}

struct ChunkIndexEntry {
    hash:   [u8; 32],
    offset: u64,
}
```

`count` adalah jumlah record chunk fisik unik yang disimpan. Setiap entri berukuran 40 byte. Ukuran total Chunk Index:

```text
4 + (count * 40) byte
```

## 10.3 Urutan Index

Entri MUST diurutkan berdasarkan:

1. `hash` secara leksikografis atas 32 byte; lalu
2. `offset` secara numerik menaik jika hash identik.

Aturan tie-break membuat urutan tetap deterministik ketika terdeteksi collision hash. Lookup berdasarkan hash memakai binary search ke rentang hash yang sama; referensi file memakai indeks entri yang tepat.

## 10.4 Makna `chunk_refs`

Nilai di kolom `chunk_refs` adalah indeks 0-based ke `ChunkIndex.entries` setelah Chunk Index final diurutkan. Nilai tersebut bukan offset fisik dan bukan nomor urut saat chunk pertama kali ditulis.

Urutan indeks dalam satu daftar file MUST mengikuti urutan byte chunk di file asli. Penulis harus membangun pemetaan record-ke-indeks setelah semua record ditulis dan Chunk Index selesai diurutkan.

## 10.5 Validasi Offset Record

Pembaca MUST memvalidasi bahwa:

- semua offset berada dalam rentang `[48, offset_table_meta)`;
- setiap offset menunjuk awal record chunk yang valid;
- tidak ada dua entri berbeda yang menunjuk record fisik yang sama;
- record tidak overlap;
- jika offset diurutkan secara numerik, record pertama mulai pada `48`, setiap record berikutnya tepat menyambung record sebelumnya, dan record terakhir berakhir tepat pada `offset_table_meta`.

Jika `count=0`, maka `offset_table_meta` harus sama dengan `48`.

---

# 11. Table Metadata

Metadata file disimpan sebagai tepat enam record kolom. Semua kolom memiliki `element_count` yang sama dengan jumlah file arsip. Urutan kolom MUST `0,1,2,3,4,5`.

## 11.1 Format Record Kolom

Format kolom v1 yang normatif:

```rust
struct Column {
    col_id:          u8,     // 0..5
    encoding:        u8,     // encoding elemen
    codec:           u8,     // 0=raw, 1=KolArk-LZ
    reserved:        u8,     // ditulis 0
    element_count:   u32,    // jumlah file
    dict_size:       u32,    // panjang dictionary dalam byte
    dictionary:      [u8; dict_size],
    compressed_size: u32,    // panjang payload setelah codec
    original_size:   u32,    // panjang payload sebelum codec
    data:            [u8; compressed_size],
}
```

Header tetap record kolom adalah 20 byte, belum termasuk dictionary dan payload. `dictionary` disimpan raw dan tidak termasuk proses kompresi `data`.

Arti field:

- `encoding` menentukan bagaimana nilai elemen direpresentasikan sebelum kompresi.
- `codec` menentukan apakah byte hasil encoding disimpan raw atau dikompresi.
- `dict_size` adalah ukuran dictionary dalam byte, bukan jumlah entry.
- `compressed_size` adalah jumlah byte `data` yang benar-benar disimpan.
- `original_size` adalah ukuran byte hasil encoding sebelum codec diterapkan.

`codec=0`: payload raw dan `compressed_size == original_size`.  
`codec=1`: payload adalah stream KolArk-LZ dan harus terdekompresi tepat menjadi `original_size` byte.  
Nilai codec lain MUST ditolak pada v1.

Untuk kolom selain kolom 0, `dict_size` MUST nol dan tidak ada byte dictionary. `reserved` ditulis nol dan diabaikan pembaca.

## 11.2 Definisi Enam Kolom

| `col_id` | Nama | Encoding | Representasi data sebelum codec |
|---:|---|---|---|
| 0 | `path_ids` | 3: dictionary + varint | Satu indeks dictionary sebagai varint per file |
| 1 | `timestamps` | 2: delta + svarint | Timestamp pertama absolut; berikutnya delta bertanda |
| 2 | `sizes` | 1: varint | Ukuran file sebagai `u64` varint |
| 3 | `modes` | 0: raw | Satu `u16` LE per file |
| 4 | `chunk_refs` | 1: varint | Untuk tiap file: jumlah chunk, lalu indeks Chunk Index |
| 5 | `content_hash` | 0: raw | Satu `hash256` per file, 32 byte |

`element_count` harus sama pada seluruh kolom dan sama dengan jumlah entri file.

## 11.3 Path Dictionary (Kolom 0)

Jika `col_id=0`, `dict_size` harus lebih besar atau sama dengan 4 dan dictionary diserialisasi sebagai:

```rust
struct PathDictionary {
    count: u32,
    entries: [string; count],
}
```

Setiap `string` adalah panjang byte varint diikuti UTF-8 path. Entry disimpan menurut urutan byte UTF-8 menaik. Path harus unik. Dengan aturan satu path per file di v1, `count` sama dengan `element_count`.

Payload `data` kolom 0 berisi tepat `element_count` varint yang masing-masing merupakan indeks 0-based ke dictionary. Setiap ID wajib berada dalam rentang `0..count`.

## 11.4 Timestamp (Kolom 1)

Timestamp adalah detik sejak Unix epoch sebagai `i64`.

Encoding:

```text
encoded[0] = svarint(timestamp[0])
encoded[i] = svarint(timestamp[i] - timestamp[i-1])  untuk i > 0
```

Decoding:

```text
timestamp[0] = decode_svarint(encoded[0])
timestamp[i] = timestamp[i-1] + decode_svarint(encoded[i])
```

Semua operasi delta dan penjumlahan harus diperiksa agar tidak overflow `i64`. Jika jumlah file nol, payload timestamp kosong.

## 11.5 Ukuran File (Kolom 2)

Setiap ukuran file disimpan sebagai `u64` varint. Ukuran adalah jumlah byte isi file asli. Jumlah chunk yang direferensikan harus dapat direkonstruksi menjadi jumlah byte yang sama.

## 11.6 Mode File (Kolom 3)

Setiap mode disimpan sebagai `u16` little-endian. V1 hanya mengizinkan entri file reguler: bit tipe file `(mode & 0xF000)` harus sama dengan `0x8000` (`S_IFREG`). Bit permission POSIX disimpan dalam 12 bit bawah.

Saat ekstraksi, bit SUID, SGID, dan sticky MUST dibersihkan secara default. Implementasi hanya boleh memulihkannya melalui opsi eksplisit dan lingkungan yang aman. Mode yang diterapkan tidak boleh memberi permission lebih tinggi dari kebijakan ekstraksi.

## 11.7 Chunk References (Kolom 4)

Untuk setiap file, data kolom berisi:

```text
chunk_count: varint
chunk_index_0: varint
chunk_index_1: varint
...
chunk_index_(chunk_count-1): varint
```

Setiap `chunk_index_n` adalah indeks 0-based ke `ChunkIndex.entries`. Decoder membatasi indeks ke rentang `0..ChunkIndex.count-1`.

Urutan referensi harus mengikuti urutan chunk pada file asli. Untuk file kosong, `chunk_count=0`. Setelah semua chunk didekompresi dan digabung, ukuran hasil wajib sama dengan kolom `sizes` dan hash wajib sama dengan kolom `content_hash`.

## 11.8 Content Hash (Kolom 5)

Kolom ini adalah concatenation dari `element_count` digest BLAKE3-256, masing-masing 32 byte. Payload sebelum codec memiliki ukuran tepat:

```text
original_size = element_count * 32
```

Perkalian harus diperiksa terhadap overflow.

## 11.9 Urutan Kolom dan Batas Metadata

Enam record kolom disimpan berurutan tanpa header table tambahan. Reader membaca tepat enam record dengan memanfaatkan `dict_size`, `compressed_size`, dan `original_size`, lalu memastikan posisi akhir sama dengan `offset_chunk_index`.

---

# 12. Kompresi KolArk-LZ

KolArk-LZ adalah codec LZ77 mandiri. Format ini bukan LZ4, zlib, gzip, atau format eksternal lain. Setiap chunk dikompresi secara independen. Setiap payload kolom juga dikompresi secara independen jika codec kolom bernilai `1`.

## 12.1 Token Stream

Stream adalah rangkaian token. Setiap token memuat literal run, lalu opsional satu match:

```text
literal_len_field: u8
if literal_len_field == 255:
    literal_len = 255 + read_varint()
else:
    literal_len = literal_len_field

literal_data: bytes(literal_len)

if EOF:
    stream selesai (literal-only final token)
else:
    offset: u16 LE
    match_len_field: u8
    if match_len_field == 255:
        match_code = 255 + read_varint()
    else:
        match_code = match_len_field
    match_len = match_code + 4
```

EOF tepat pada batas token setelah match juga mengakhiri stream yang valid. EOF di tengah field, literal, offset, atau extension varint adalah kesalahan.

## 12.2 Makna Token

- `literal_len` adalah jumlah byte literal yang langsung disalin ke output.
- `offset` adalah jarak mundur dari akhir output saat ini ke awal data match.
- `offset` wajib berada dalam rentang `1..=65535` dan tidak boleh melebihi panjang output yang sudah terbentuk.
- `match_len` minimum 4 byte.
- Match boleh overlap. Decoder menyalin byte demi byte dari posisi `output.len() - offset`, sehingga pola berulang dapat memperluas dirinya sendiri.
- Match hanya boleh merujuk output chunk/kolom yang sedang didekompresi, tidak boleh merujuk blok lain.

## 12.3 Batas dan Validasi Decoder

Decoder MUST:

1. Menolak offset nol.
2. Menolak offset yang lebih besar dari output yang tersedia.
3. Menolak varint extension yang overflow atau nonminimal.
4. Menolak literal atau match yang menyebabkan output melampaui `original_size` yang dideklarasikan.
5. Berhenti tepat ketika output mencapai `original_size` dan stream habis; byte trailing tambahan adalah kesalahan.
6. Memastikan output final tepat sama dengan `original_size`.

## 12.4 Encoder Canonical

Agar Python dan Rust menghasilkan payload identik, encoder v1 MUST memakai aturan greedy deterministik berikut:

1. Pada posisi input saat ini, cari semua offset dari `1` hingga `min(position, 65535)`.
2. Untuk setiap offset, hitung panjang match terpanjang yang cocok dengan input, termasuk match overlap, sampai akhir blok.
3. Pilih match terpanjang dengan panjang minimal 4 byte.
4. Jika terdapat panjang sama, pilih offset numerik paling kecil (match terdekat).
5. Jika tidak ada match minimal 4 byte, majukan posisi satu byte dan lanjutkan pencarian.
6. Saat match ditemukan, emit satu token yang berisi literal sejak akhir match sebelumnya sampai posisi match, kemudian offset dan panjang match; lanjutkan posisi melewati match.
7. Setelah mencapai akhir input, emit token literal-only untuk sisa literal jika ada. Jika match terakhir berakhir tepat di EOF, stream boleh berakhir tepat setelah match.

Implementasi boleh menggunakan struktur indeks untuk mempercepat pencarian, tetapi hasilnya MUST identik dengan aturan di atas.

## 12.5 Representasi Panjang

- Literal length `0..254` disimpan langsung dalam satu byte.
- Literal length `>=255` disimpan sebagai `FF` diikuti `varint(literal_len - 255)`.
- Match code `0..254` mewakili panjang `4..258`.
- Match length `>=259` disimpan dengan match code `FF`, diikuti `varint(match_len - 4 - 255)`.

## 12.6 Pilihan Raw atau LZ

Untuk chunk: jika ukuran stream LZ kurang dari ukuran byte asli, penulis MAY menyimpan LZ (`flags=0`). Jika ukuran LZ lebih besar atau sama, penulis MUST menyimpan raw (`flags=1`).

Untuk kolom: jika ukuran stream LZ kurang dari ukuran data kolom sebelum codec, penulis MUST memakai `codec=1`; jika tidak, MUST memakai `codec=0`. Kebijakan ini membuat pilihan codec canonical.

---

# 13. Footer (72 Byte) dan Prosedur Pembacaan

Footer selalu berada di akhir file dan memiliki panjang tetap 72 byte.

```rust
struct Footer {
    offset_superblock:  u64,       // selalu 16
    offset_chunk_store: u64,       // selalu 48
    offset_table_meta:  u64,
    offset_chunk_index: u64,
    hash_all:           [u8; 32],  // BLAKE3 byte 0 sampai footer_start-1
    footer_len:         u32,       // selalu 72
    magic:              [u8; 4],  // b"KOLA"
}
```

Ukuran:

```text
4 * u64       = 32 byte
hash256       = 32 byte
footer_len    =  4 byte
magic         =  4 byte
TOTAL         = 72 byte
```

Offset field footer relatif terhadap awal arsip. `offset_superblock` wajib 16 dan `offset_chunk_store` wajib 48.

## 13.1 Pembacaan Footer

1. Dapatkan ukuran file melalui operasi seek/stat yang aman.
2. Tolak file yang lebih kecil dari 120 byte.
3. Baca empat byte terakhir dan pastikan sama dengan `KOLA`.
4. Baca `footer_len` pada empat byte sebelum magic dan pastikan nilainya 72.
5. Baca seluruh footer dari `file_size - 72`.
6. Validasi offset tetap dan relasi offset dinamis.
7. Hitung BLAKE3 atas byte `[0, file_size - 72)` dan bandingkan dengan `hash_all`.
8. Jika hash tidak cocok, gagalkan validasi arsip sebelum ekstraksi.
9. Parse Chunk Index, lalu Table Metadata dan Chunk Store menggunakan offset yang telah divalidasi.

`hash_all` tidak mencakup footer itu sendiri. Footer dilindungi secara tidak langsung oleh validasi struktur dan nilai hash yang disimpan di dalam footer, tetapi bukan autentikasi kriptografis.

---

# 14. Aturan Penulisan Arsip Kanonis

Penulis conforming MUST mengikuti urutan dan aturan berikut agar output untuk input logis yang sama dapat direproduksi lintas bahasa.

## 14.1 Pemilihan File dan Urutan Baris

- Hanya file reguler yang dimasukkan.
- Path harus berupa bentuk relatif dengan separator `/`; penulis tidak boleh mengubah byte Unicode melalui normalisasi NFC/NFD.
- Path harus lolos aturan keamanan §16.3.
- Semua entri diurutkan berdasarkan byte UTF-8 path secara leksikografis menaik.
- Dua path identik dilarang.

## 14.2 Waktu dan Mode

- Timestamp yang disimpan adalah detik Unix integer (`i64`). Jika sumber memiliki resolusi lebih tinggi, bagian subdetik dibuang dengan pembulatan ke bawah menuju detik.
- Mode adalah nilai POSIX `st_mode` 16-bit; entri harus berupa file reguler.
- Implementasi pada platform non-POSIX harus menggunakan pemetaan mode yang ditentukan aplikasinya secara konsisten.

## 14.3 Urutan Serialisasi

Penulis MUST menulis:

1. Header (16 byte).
2. Superblock (32 byte).
3. Chunk Store, record sesuai urutan pertama kali chunk unik ditemukan saat file diproses secara urutan path canonical dan byte file dibaca berurutan.
4. Table Metadata, kolom `0` sampai `5`.
5. Chunk Index, entri diurutkan sesuai §10.3.
6. Footer, ditulis paling akhir.

Chunk referensi untuk baris file dibentuk kembali ke indeks akhir Chunk Index setelah pengurutan indeks.

## 14.4 Perhitungan Hash

- `chunk_hash`: BLAKE3 byte chunk asli.
- `content_hash`: BLAKE3 byte file asli secara streaming.
- `hash_all`: BLAKE3 semua byte dari offset 0 sampai byte sebelum footer.

Penulis harus menyelesaikan seluruh bagian sebelum menghitung `hash_all`. Footer baru dapat ditulis setelah hash body diperoleh.

## 14.5 Penulisan Aman

Penulis SHOULD menulis ke file sementara, melakukan flush/sync sesuai kebutuhan durability, kemudian melakukan rename atomik ke tujuan. Arsip final tidak boleh ditinggalkan dalam keadaan setengah tertulis jika operasi gagal.

---

# 15. Prosedur Pembacaan dan Ekstraksi

Reader conforming melakukan validasi berlapis sebelum mengekstrak.

## 15.1 Validasi Struktur

1. Validasi ukuran file, header, magic, versi, flags.
2. Baca Superblock dan validasi algoritma serta parameter chunk.
3. Baca Footer dari akhir file dan validasi panjang, magic, serta offset.
4. Verifikasi `hash_all` sebelum ekstraksi.
5. Parse Chunk Index; validasi jumlah, urutan, offset, dan ukuran record.
6. Parse tepat enam record Table Metadata; validasi ID, encoding, codec, jumlah elemen, ukuran, dan akhir table.
7. Validasi seluruh path sebelum membuat file tujuan.
8. Validasi seluruh referensi chunk terhadap Chunk Index.

## 15.2 Ekstraksi File

Untuk setiap baris metadata sesuai urutan canonical:

1. Baca path ID dan resolve path dari dictionary.
2. Buat direktori induk dengan mekanisme aman yang tidak mengikuti symlink keluar target.
3. Baca daftar indeks chunk file.
4. Untuk setiap indeks, baca record chunk yang ditunjuk.
5. Dekompress jika diperlukan dan verifikasi hash chunk.
6. Gabungkan byte chunk sesuai urutan referensi.
7. Pastikan ukuran gabungan sama dengan kolom `sizes`.
8. Hitung BLAKE3 isi gabungan dan bandingkan dengan `content_hash`.
9. Tulis file sementara di direktori tujuan, flush bila diperlukan, lalu rename atomik.
10. Terapkan mode aman berdasarkan kebijakan ekstraksi dan timestamp jika didukung platform.

Pembaca MUST menolak file yang memiliki jumlah byte atau hash isi tidak cocok.

## 15.3 Deduplikasi Saat Ekstraksi

Implementasi MAY memakai hardlink atau reflink untuk file dengan konten identik. Hardlink hanya boleh dipakai bila metadata inode yang akan dibagi kompatibel, setidaknya mode dan timestamp yang perlu dipulihkan sama, serta sumber dan tujuan berada pada filesystem yang mendukung operasi tersebut. Karena hardlink berbagi inode, implementasi MUST NOT mengubah permission atau timestamp pada salah satu path setelah membuat hardlink jika perubahan tersebut akan memengaruhi path lain.

Jika syarat tersebut tidak terpenuhi, gunakan reflink bila aman dan tersedia, atau salin byte secara normal. Kegagalan hardlink/reflink harus kembali ke salinan normal.

---

# 16. Integritas, Keamanan, dan Batas Sumber Daya

## 16.1 Integritas

Reader MUST memeriksa:

- `hash_all` atas seluruh body arsip sebelum ekstraksi;
- hash setiap chunk atas byte hasil dekompresi;
- hash setiap file setelah semua chunk digabung;
- ukuran aktual chunk dan file terhadap ukuran yang dideklarasikan.

Pemeriksaan hash tidak mencegah pemalsuan oleh pihak yang dapat menulis ulang arsip dan menghitung ulang hash. Untuk autentikasi penerbit diperlukan tanda tangan digital atau MAC yang tidak termasuk v1.

## 16.2 Validasi Panjang dan Overflow

Sebelum alokasi atau pembacaan:

- semua `offset + length` harus diperiksa overflow;
- `count * entry_size` harus diperiksa overflow;
- `original_size` chunk tidak boleh melebihi `chunk_size_max`;
- ukuran hasil dekompresi kolom tidak boleh melebihi batas resource aplikasi;
- jumlah file, chunk, path, dan byte output harus tunduk pada konfigurasi batas reader;
- decoder varint harus menolak overflow dan encoding nonminimal;
- reader tidak boleh mempercayai panjang terkompresi sebagai ukuran hasil dekompresi.

Batas teoritis field tidak berarti implementasi wajib menerima arsip sebesar itu. Reader boleh menetapkan batas yang lebih rendah dan melaporkannya sebagai resource-limit error, bukan sebagai kerusakan format.

## 16.3 Keamanan Path

Path pada dictionary MUST:

- berupa UTF-8 valid;
- relatif, tidak diawali `/`;
- menggunakan `/` sebagai separator;
- tidak mengandung komponen kosong, `.` atau `..`;
- tidak mengandung NUL atau backslash `\\`;
- tidak menggunakan prefix drive atau skema absolut;
- tidak melebihi 4096 byte.

Unpacker MUST memastikan hasil resolusi path tetap berada di dalam direktori target. Pemeriksaan string sederhana saja tidak cukup jika direktori tujuan berisi symlink yang dapat mengalihkan penulisan; implementasi harus memakai operasi filesystem yang aman terhadap symlink (misalnya dir-relative/openat dengan no-follow semantics pada platform yang mendukungnya) atau mekanisme ekuivalen.

Path duplikat MUST ditolak. Collision setelah pemetaan case-insensitive pada filesystem target juga harus ditangani tanpa menimpa file secara diam-diam.

## 16.4 Permission dan Tipe File

V1 hanya mengizinkan file reguler. Reader MUST menolak mode yang menyatakan tipe file lain. Bit SUID/SGID/sticky dibersihkan secara default saat ekstraksi kecuali kebijakan eksplisit mengizinkan dan lingkungan aman.

## 16.5 Ekstraksi Aman

- Jangan menimpa file yang sudah ada tanpa kebijakan eksplisit.
- Gunakan file sementara lalu rename atomik.
- Jangan mengikuti symlink dari direktori target ke luar target.
- Jangan menerapkan mode yang lebih permisif daripada kebijakan pengguna.
- Hentikan proses jika integritas arsip/chunk/file gagal.

---

# 17. Batasan Versi 1

| Parameter | Batas format | Catatan |
|---|---:|---|
| Ukuran file arsip | `u64` | Dibatasi filesystem dan implementasi |
| Jumlah file | `u32::MAX` secara field; lebih rendah menurut ukuran kolom | Kolom `content_hash` dibatasi `original_size: u32`, sehingga maksimum teoritis kolom ini adalah `floor(u32::MAX / 32)` file |
| Jumlah chunk | `u32::MAX` | Dari `ChunkIndex.count` |
| Ukuran chunk asli | `u32` | Juga dibatasi `chunk_size_max` |
| Ukuran path | 4096 byte | Batas keras v1 |
| Jumlah dictionary entry | `u32::MAX` | Dibatasi count dictionary |
| Offset | `u64` | Dari awal file |
| Mode | `u16` | POSIX mode 16-bit |
| Timestamp | `i64` | Detik Unix |

Setiap kolom menyimpan `original_size` dan `compressed_size` sebagai `u32`; karena itu ukuran hasil encoding tiap kolom tidak boleh melebihi `u32::MAX` byte. Batas praktis jumlah file dapat lebih rendah dari batas `element_count`, misalnya karena kolom `content_hash` membutuhkan 32 byte per file. Implementasi MUST menerapkan batas operasional untuk mencegah konsumsi memori/disk berlebihan, meskipun batas format lebih tinggi.

---

# 18. Interoperabilitas dan Konformansi

Dua implementasi conforming untuk versi biner 1 wajib:

- membaca dan menulis field dengan ukuran dan endianness identik;
- menerapkan varint dan svarint kanonis;
- membangkitkan tabel Gear identik;
- menghasilkan batas CDC identik;
- menerapkan aturan deduplikasi yang sama;
- menghasilkan urutan file, dictionary, chunk index, dan kolom sesuai aturan canonical;
- memakai encoder KolArk-LZ canonical;
- menghasilkan byte arsip identik untuk input metadata, file, dan urutan parameter yang identik.

## 18.1 Test yang Wajib

Minimal, implementasi harus menguji:

1. Header valid, magic salah, versi salah, flags tidak didukung.
2. Superblock parameter valid dan tidak valid.
3. Varint batas 0, 127, 128, `u32::MAX`, `u64::MAX`, nonminimal, terpotong, dan overflow.
4. Zigzag nilai 0, -1, 1, `i64::MIN`, `i64::MAX`.
5. CDC untuk input kosong, input lebih kecil dari minimum, boundary alami, dan force cut maksimum.
6. Dedup chunk identik lintas file dan beberapa kemunculan dalam file yang sama.
7. Chunk raw dan chunk LZ.
8. Kolom codec raw dan LZ.
9. Chunk Index dengan hash berurutan dan collision hash simulasi.
10. File kosong, satu file, banyak file, dan file dengan banyak chunk.
11. Path traversal, path absolut, komponen kosong, backslash, NUL, dan collision nama pada target.
12. Footer rusak, offset overflow, overlap record, data terpotong, dan hash mismatch.
13. Python/Rust cross-read dan byte-for-byte cross-write.

---

# 19. Test Vector

## 19.1 Input Canonical

```text
path:        hello.txt
contents:    "Hello, World!\n" (14 byte)
mtime:       1700000000 (Unix epoch seconds)
mode:        0o100644 (regular file, rw-r--r--)
chunk_min:   16384
chunk_avg:   65536
chunk_max:   262144
hash_alg:    0 (BLAKE3-256)
compression: 0 (KolArk-LZ)
```

Karena isi file lebih kecil dari `chunk_size_min`, CDC menghasilkan satu chunk akhir sepanjang 14 byte. Stream KolArk-LZ literal-only lebih besar daripada 14 byte, sehingga chunk disimpan raw (`flags=1`). Payload kolom pada test vector ini juga disimpan raw (`codec=0`) bila stream LZ tidak lebih kecil.

## 19.2 Known-answer BLAKE3

Implementasi BLAKE3 wajib lulus setidaknya:

```text
BLAKE3("") = af1349b9f5f9a1a6a0404dea36dcc949
               9bcb25c9adc112b7cc9a93cae41f3262

BLAKE3("abc") = 6437b3ac38465133ffb63b75273a8db5
                 48c558465d79db03fd359c6cd5bd9d85

BLAKE3("Hello, World!\n") = 6233834bce7817db1e401bceea2b8b45
                               c1600115d25b51906c05ee32156b51c2
```

## 19.3 Byte-for-byte Archive Vector

Berkas pendamping `kolark_golden_hello.kola` dan `kolark_golden_hello.hex` adalah vector untuk input §19.1 berdasarkan layout revisi ini. Ukuran arsip adalah **396 byte**; offset Table Metadata `103`, offset Chunk Index `280`, dan awal Footer `324`. Nilai `hash_all` adalah `485014c836d06ba252e87ebeeaa3751511d519b08ee895b41808777d53799a31`. Generator pendamping `kolark_golden_vector.py` menjelaskan serialisasi dengan Python standard library dan implementasi BLAKE3 yang diuji terhadap known-answer BLAKE3 di atas.

Implementasi Rust wajib membaca vector tersebut dan menghasilkan file hasil ekstraksi yang byte-identik dengan input. Penulis Python dan Rust wajib menghasilkan arsip byte-identik dengan parameter dan metadata yang sama.

Golden vector ini menguji satu file kecil dan belum menggantikan stress test CDC, deduplikasi, LZ, korupsi, atau keamanan path.

---

# 20. Ringkasan Struktur Biner

## 20.1 Header

| Field | Tipe | Ukuran |
|---|---|---:|
| magic | bytes(4) | 4 |
| version | u32 | 4 |
| flags | u32 | 4 |
| reserved | u32 | 4 |
| **Total** | | **16** |

## 20.2 Superblock

| Field | Tipe | Ukuran |
|---|---|---:|
| chunk_size_min | u32 | 4 |
| chunk_size_avg | u32 | 4 |
| chunk_size_max | u32 | 4 |
| hash_alg | u8 | 1 |
| compression | u8 | 1 |
| flags | u16 | 2 |
| reserved | bytes(16) | 16 |
| **Total** | | **32** |

## 20.3 Chunk Record Header

| Field | Tipe | Ukuran |
|---|---|---:|
| hash | hash256 | 32 |
| size_compressed | u32 | 4 |
| size_original | u32 | 4 |
| flags | u8 | 1 |
| **Total header** | | **41** |

Payload mengikuti header sepanjang `size_compressed`.

## 20.4 Column Fixed Header

| Field | Tipe | Ukuran |
|---|---|---:|
| col_id | u8 | 1 |
| encoding | u8 | 1 |
| codec | u8 | 1 |
| reserved | u8 | 1 |
| element_count | u32 | 4 |
| dict_size | u32 | 4 |
| compressed_size | u32 | 4 |
| original_size | u32 | 4 |
| **Total fixed fields** | | **20** |

Dictionary bytes (jika ada) disimpan setelah `dict_size` dan sebelum field `compressed_size`, `original_size`, serta payload `data`. Payload muncul setelah seluruh field ukuran.

## 20.5 Chunk Index Entry

| Field | Tipe | Ukuran |
|---|---|---:|
| hash | hash256 | 32 |
| offset | u64 | 8 |
| **Total** | | **40** |

Chunk Index diawali `count: u32`.

## 20.6 Footer

| Field | Tipe | Ukuran |
|---|---|---:|
| offset_superblock | u64 | 8 |
| offset_chunk_store | u64 | 8 |
| offset_table_meta | u64 | 8 |
| offset_chunk_index | u64 | 8 |
| hash_all | hash256 | 32 |
| footer_len | u32 | 4 |
| magic | bytes(4) | 4 |
| **Total** | | **72** |

---

# Lampiran A. Algoritma Pembangkitan Tabel Gear

Tabel Gear MUST dihasilkan dengan SplitMix64 berikut, lalu mengambil 32 bit rendah setiap output. Semua operasi state dan perkalian dilakukan modulo `2^64`.

Konstanta:

```text
SEED      = 0x4B6F6C41726B0001
GAMMA     = 0x9E3779B97F4A7C15
MIX1      = 0xBF58476D1CE4E5B9
MIX2      = 0x94D049BB133111EB
MASK64    = 0xFFFFFFFFFFFFFFFF
MASK32    = 0x00000000FFFFFFFF
```

Pseudocode:

```text
state = SEED
for i in 0..255:
    state = (state + GAMMA) & MASK64
    z = state
    z = ((z XOR (z >> 30)) * MIX1) & MASK64
    z = ((z XOR (z >> 27)) * MIX2) & MASK64
    z = z XOR (z >> 31)
    GEAR_TABLE_256[i] = z & MASK32
```

Hasilnya adalah tepat 256 nilai `u32`. Implementasi tidak boleh memakai seed lain atau mengubah urutan operasi. Implementasi yang menyimpan tabel literal harus memastikan nilainya identik dengan hasil algoritma di atas.

---

# Lampiran B. Catatan Keputusan Pembakuan

Keputusan berikut dibuat untuk menghilangkan ambiguitas draf yang diberikan:

1. **Footer 72 byte dipertahankan.** Empat `u64` offset + hash 32 byte + panjang 4 byte + magic 4 byte tepat berjumlah 72 byte.
2. **Superblock 32 byte dipertahankan.** Tiga `u32`, dua `u8`, satu `u16`, dan 16 reserved byte tepat berjumlah 32 byte.
3. **Chunk header 41 byte dipertahankan.** Field `flags` 1 byte adalah bagian record; payload langsung mengikuti.
4. **Gear table lama tidak dipakai.** Tabel literal di draf memiliki 376 entri dan hanya 31 nilai unik, bukan 256 nilai yang dinyatakan. Tabel v1 kini dibangkitkan deterministik melalui Lampiran A.
5. **Istilah sliding window dihapus.** Pseudocode sebelumnya tidak mengimplementasikan jendela geser byte. V1 menggunakan Gear state yang diperbarui per byte dan direset saat batas chunk.
6. **Rumus boundary dibakukan.** `chunk_size_avg - chunk_size_min` menjadi rentang ekspektasi setelah minimum, sehingga parameter rata-rata mempunyai makna yang lebih konsisten.
7. **Field codec kolom ditambahkan.** Encoding elemen dan kompresi payload adalah dua lapisan berbeda. Karena itu record kolom memiliki `codec` byte sendiri dan `reserved` byte; fixed header kolom menjadi 20 byte.
8. **Urutan chunk index diberi tie-break.** Hash diurutkan leksikografis, lalu offset menaik agar collision hash tetap dapat direpresentasikan.
9. **Referensi chunk menunjuk entri Chunk Index final.** Referensi bukan offset fisik dan bukan urutan penemuan chunk.
10. **Encoder KolArk-LZ dibuat canonical.** Aturan match terpanjang dan tie-break offset terkecil diperlukan agar hasil Python dan Rust byte-identik.
11. **Versi dibaca secara exact.** Reader v1 menerima hanya `version=1`, bukan sekadar menolak nilai yang lebih besar dari 1.
12. **Cakupan file v1 dipersempit eksplisit.** Hanya file reguler; symlink dan special file tidak disimpan. Direktori induk dibuat otomatis saat ekstraksi.

Dokumen ini adalah spesifikasi normatif untuk pembakuan KolArk v1.0.0. Kesesuaian implementasi tetap harus dibuktikan dengan golden vector, pengujian lintas bahasa, uji kerusakan, dan audit keamanan.

---

# BAGIAN II — SPESIFIKASI PRODUK KOLARK

Bagian ini melengkapi spesifikasi format biner dengan persyaratan produk, antarmuka pengguna, arsitektur implementasi, kualitas, distribusi, dan kriteria rilis. Bagian I tetap menjadi sumber normatif untuk byte format `.kola`; jika terdapat konflik, aturan format biner pada Bagian I mengatur interpretasi arsip.

# 21. Definisi Produk

## 21.1 Nama dan Kategori

- **Nama produk:** KolArk
- **Kepanjangan:** Columnar Archive
- **Kategori:** format arsip, pustaka kompresi/deduplikasi, dan perangkat CLI.
- **Format arsip:** `.kola`
- **Magic:** `KOLA`
- **Versi format:** binary version `1`
- **Versi dokumen ini:** product specification `1.0.0`
- **Bahasa utama dokumentasi:** Bahasa Indonesia; istilah teknis dan identifier API menggunakan bahasa Inggris.

## 21.2 Pernyataan Produk

KolArk adalah sistem pengarsipan yang menggabungkan content-defined chunking, deduplikasi berbasis konten, kompresi blok, metadata kolumnar, indeks di bagian akhir arsip, dan verifikasi integritas. Produk menyediakan format biner terbuka, implementasi referensi, CLI, dan pustaka yang dapat digunakan aplikasi lain.

KolArk bukan sekadar wrapper untuk ZIP/TAR dan tidak mengklaim kompatibilitas dengan format tersebut. Konversi ke/dari format lain merupakan fungsi aplikasi tambahan dan tidak mengubah format inti `.kola`.

## 21.3 Sasaran Pengguna

- Pengembang perangkat lunak yang membutuhkan arsip terdeduplikasi.
- Operator sistem dan administrator yang mengarsipkan kumpulan file.
- Sistem backup dan sinkronisasi yang memerlukan chunking stabil.
- Aplikasi yang membutuhkan validasi integritas arsip.
- Pengembang yang membutuhkan pustaka Rust dan binding lintas bahasa.

## 21.4 Prinsip Produk

1. **Deterministik:** input dan opsi yang sama menghasilkan byte arsip yang sama pada implementasi conforming.
2. **Terbuka:** format didokumentasikan lengkap dan tidak bergantung pada layanan cloud.
3. **Aman secara default:** reader memvalidasi struktur, panjang, hash, path, dan batas sumber daya.
4. **Dapat diuji:** golden vector, fixture, fuzz target, dan suite interoperabilitas menjadi bagian proyek.
5. **Modular:** format, codec, pustaka, CLI, dan integrasi aplikasi dipisahkan.
6. **Tidak diam-diam merusak data:** kegagalan validasi menghentikan operasi dan tidak menghasilkan ekstraksi parsial yang dianggap berhasil.

# 22. Ruang Lingkup Produk dan Batas Fitur

## 22.1 Fitur Rilis Awal (MUST)

- Membuat arsip `.kola` dari file reguler.
- Membaca dan memvalidasi arsip `.kola` versi 1.
- Menampilkan daftar isi tanpa mengekstrak.
- Mengekstrak file dengan validasi path dan integritas.
- Memverifikasi arsip secara menyeluruh.
- Menghitung statistik arsip.
- Menggunakan CDC dan deduplikasi chunk sesuai spesifikasi.
- Menggunakan BLAKE3-256.
- Menggunakan KolArk-LZ sesuai grammar dan encoder canonical.
- Menghasilkan error yang terstruktur dan exit code yang stabil.
- Mendukung operasi stdin/stdout jika implementasi dapat melakukannya tanpa melanggar kebutuhan seek/footer; untuk pembuatan arsip non-seekable, writer wajib melakukan spooling atau menolak dengan pesan yang jelas.
- Memiliki mode uji konformansi untuk golden vectors.

## 22.2 Fitur yang Dapat Ditambahkan Setelah Rilis Awal

Fitur berikut tidak boleh mengubah byte format v1 tanpa revisi spesifikasi:

- Progress bar dan estimasi waktu.
- Parallel hashing dan kompresi, selama urutan output canonical tetap sama.
- Cache chunk sementara.
- Reflink/hardlink saat ekstraksi.
- Integrasi backup incremental.
- Binding Python, C ABI, dan bahasa lain.
- GUI.
- Plugin atau adapter storage.

## 22.3 Di Luar Cakupan Format v1

- Enkripsi dan manajemen kunci.
- Tanda tangan digital dan identitas penerbit.
- Symlink, hardlink sebagai entri arsip, special file, ACL, xattr, owner/group.
- Multi-volume archive.
- Streaming format tanpa footer.
- Random-access index tambahan di luar struktur yang ditentukan.
- Kompresi codec selain KolArk-LZ untuk chunk dan codec yang ditentukan metadata.
- Delta update/in-place mutation arsip.

Fitur di luar cakupan dapat menjadi proposal versi format berikutnya atau lapisan aplikasi. Implementasi v1 tidak boleh menyisipkan ekstensi privat ke area reserved atau flags.

# 23. Antarmuka CLI Normatif Produk

Nama executable yang direkomendasikan adalah `kolark`. Nama subcommand dan opsi berikut merupakan kontrak antarmuka produk, bukan bagian dari byte format.

## 23.1 Perintah

| Perintah | Fungsi |
|---|---|
| `kolark create <archive.kola> <paths...>` | Membuat arsip |
| `kolark list <archive.kola>` | Menampilkan isi arsip |
| `kolark extract <archive.kola> [-C DIR]` | Mengekstrak arsip |
| `kolark verify <archive.kola>` | Memvalidasi struktur dan seluruh hash |
| `kolark info <archive.kola>` | Menampilkan versi, ukuran, jumlah file/chunk, dan parameter |
| `kolark test-vector` | Menjalankan fixture konformansi yang dibundel |
| `kolark version` | Menampilkan versi aplikasi dan versi format yang didukung |

## 23.2 Opsi Umum

| Opsi | Makna |
|---|---|
| `-q, --quiet` | Mengurangi output non-error |
| `-v, --verbose` | Menampilkan detail proses |
| `--json` | Menghasilkan output mesin dalam JSON |
| `--no-progress` | Menonaktifkan progress |
| `--help` | Menampilkan bantuan |
| `--version` | Menampilkan versi |

## 23.3 Opsi Create

| Opsi | Makna |
|---|---|
| `-f, --force` | Mengizinkan penggantian tujuan sesuai kebijakan eksplisit |
| `--chunk-min <bytes>` | Parameter minimum CDC |
| `--chunk-avg <bytes>` | Parameter target rata-rata CDC |
| `--chunk-max <bytes>` | Parameter maksimum CDC |
| `--mtime <policy>` | Kebijakan timestamp; default preserve detik Unix |
| `--mode <policy>` | Kebijakan mode; default preserve mode reguler yang aman |
| `--deterministic` | Mengaktifkan seluruh aturan output canonical; default aktif |
| `--follow-symlinks` | Tidak tersedia pada v1; harus ditolak sebagai opsi tidak didukung |

Opsi parameter CDC hanya boleh mengubah nilai Superblock dan hasil chunking sesuai algoritma v1. Opsi tersebut tidak boleh mengganti algoritma Gear, seed, hash, atau codec.

## 23.4 Opsi Extract

| Opsi | Makna |
|---|---|
| `-C, --directory <DIR>` | Direktori target |
| `--overwrite` | Mengizinkan overwrite sesuai kebijakan eksplisit |
| `--no-preserve-mode` | Tidak menerapkan mode arsip |
| `--no-preserve-time` | Tidak menerapkan timestamp arsip |
| `--strip-components <N>` | Menghapus N komponen awal path; hasil tetap harus aman |
| `--dry-run` | Memvalidasi dan menampilkan rencana tanpa menulis file |
| `--max-output-bytes <N>` | Batas total byte hasil ekstraksi |
| `--max-files <N>` | Batas jumlah file |
| `--max-memory <N>` | Batas memori operasional |

Ekstraksi tanpa opsi overwrite tidak boleh menimpa file yang telah ada. `--dry-run` tetap menjalankan validasi struktur, referensi, dan hash sesuai tingkat verifikasi yang dijanjikan perintah.

## 23.5 Exit Code

| Kode | Arti |
|---:|---|
| 0 | Berhasil |
| 1 | Kesalahan umum |
| 2 | Argumen atau penggunaan tidak valid |
| 3 | Arsip rusak atau format tidak valid |
| 4 | Versi/fitur format tidak didukung |
| 5 | Kegagalan I/O atau filesystem |
| 6 | Batas sumber daya terlampaui |
| 7 | Kegagalan integritas/hash |
| 8 | Konflik tujuan atau kebijakan overwrite |

Kode harus konsisten di seluruh platform. Error output untuk manusia ditulis ke stderr; output data/JSON ditulis ke stdout.

# 24. API Pustaka

Pustaka inti menyediakan API tingkat tinggi dan API tingkat rendah. Nama berikut adalah nama konseptual yang harus dipetakan konsisten pada crate Rust.

## 24.1 API Tingkat Tinggi

```rust
pub fn create_archive(
    output: impl Write + Seek,
    inputs: &[InputEntry],
    options: &CreateOptions,
) -> Result<ArchiveSummary, KolArkError>;

pub fn open_archive(
    input: impl Read + Seek,
    options: &ReaderOptions,
) -> Result<ArchiveReader, KolArkError>;

impl ArchiveReader {
    pub fn entries(&self) -> &[ArchiveEntry];
    pub fn info(&self) -> &ArchiveInfo;
    pub fn verify(&mut self, level: VerifyLevel) -> Result<VerifyReport, KolArkError>;
    pub fn extract_all(
        &mut self,
        destination: impl AsRef<Path>,
        options: &ExtractOptions,
    ) -> Result<ExtractReport, KolArkError>;
}
```

Signature ini adalah kontrak desain. Detail lifetime, trait object, async, dan generic dapat disesuaikan selama semantik dan jaminan error tidak berubah.

## 24.2 API Tingkat Rendah

Modul internal/publik yang direkomendasikan:

- `format`: tipe header, superblock, footer, column, chunk record.
- `codec::blake3`: hashing dan known-answer tests.
- `codec::gear`: pembangkitan tabel dan CDC.
- `codec::lz`: encoder/decoder KolArk-LZ.
- `metadata`: encode/decode enam kolom.
- `index`: pembuatan, sorting, dan validasi Chunk Index.
- `reader`: parsing, validasi, random access internal.
- `writer`: canonical archive writer.
- `extract`: path-safe extraction.
- `limits`: kebijakan resource limit.
- `error`: tipe error publik.
- `cli`: antarmuka command line, berada di binary terpisah dari library.

## 24.3 Tipe Error

Error publik minimal membedakan:

- `Io`
- `InvalidMagic`
- `UnsupportedVersion`
- `UnsupportedFlags`
- `InvalidSuperblock`
- `InvalidFooter`
- `InvalidOffset`
- `IntegerOverflow`
- `InvalidVarint`
- `InvalidEncoding`
- `InvalidCodec`
- `DecompressionError`
- `HashMismatch`
- `InvalidChunkIndex`
- `InvalidMetadata`
- `InvalidPath`
- `PathCollision`
- `ResourceLimitExceeded`
- `DestinationExists`
- `UnsupportedFeature`

Error tidak boleh menyertakan secret karena format v1 tidak memiliki secret; pesan error tetap harus menghindari kebocoran path sensitif bila aplikasi memilih mode redaksi.

# 25. Arsitektur Implementasi Referensi

## 25.1 Struktur Workspace

Struktur workspace Rust yang direkomendasikan:

```text
kolark/
├── Cargo.toml
├── crates/
│   ├── kolark-format/       # tipe dan serialisasi format
│   ├── kolark-codec/        # BLAKE3, Gear CDC, KolArk-LZ
│   ├── kolark-core/         # reader, writer, metadata, index
│   ├── kolark-extract/      # ekstraksi aman
│   └── kolark-cli/          # executable kolark
├── tests/
│   ├── conformance/
│   ├── interoperability/
│   ├── corruption/
│   └── security/
├── test-vectors/
│   ├── valid/
│   ├── invalid/
│   └── manifests/
├── fuzz/
├── docs/
├── benches/
└── README.md
```

Pemisahan crate boleh disederhanakan pada fase awal, tetapi batas tanggung jawab modul harus tetap jelas. Tidak boleh ada circular dependency.

## 25.2 Prinsip Implementasi

- Gunakan Rust stable dan edition yang dipatok dalam `rust-toolchain.toml`.
- Hindari `unsafe` kecuali ada kebutuhan terukur, review keselamatan, dan test khusus.
- Semua parser memakai pembacaan bounded; jangan membaca panjang dari arsip lalu langsung mengalokasikan tanpa validasi limit.
- Semua arithmetic offset/size memakai checked arithmetic.
- Tidak menggunakan `unwrap()`/`expect()` pada jalur input arsip yang tidak tepercaya.
- Jangan menyimpan seluruh isi arsip di RAM jika operasi streaming dapat digunakan.
- Pemrosesan paralel tidak boleh mengubah urutan serialisasi canonical.
- Dependency harus dipatok dan diaudit; tidak boleh mengunduh dependency saat runtime.
- Hindari dependency ganda untuk fungsi yang sama tanpa alasan teknis.
- Jangan memasukkan placeholder, stub, hardcoded test-only path, atau fitur yang hanya mengembalikan sukses tanpa implementasi.

## 25.3 Dependency Policy

Dependency dipilih berdasarkan kebutuhan, pemeliharaan aktif, lisensi kompatibel, keamanan, dan ukuran dependency tree. BLAKE3 dapat memakai implementasi crate yang telah diaudit, tetapi known-answer tests wajib tetap dijalankan. Gear CDC dan KolArk-LZ harus mengikuti algoritma format; dependency kompresi eksternal tidak boleh menggantikan KolArk-LZ.

Setiap dependency langsung dan transitif harus tercatat dalam lockfile. Rilis harus dapat dibangun secara reproducible dari source dan lockfile. Audit dependency dilakukan pada CI dan sebelum rilis.

# 26. Persyaratan Non-Fungsional

## 26.1 Correctness

- Reader menolak semua struktur yang melanggar spesifikasi.
- Writer menghasilkan struktur canonical.
- Round-trip `create -> verify -> extract` harus mempertahankan byte file.
- Python dan Rust reference implementation harus saling membaca arsip.
- Output writer lintas implementasi harus identik untuk fixture canonical yang sama.

## 26.2 Keamanan

- Arsip dianggap input tidak tepercaya.
- Fail closed untuk format, offset, ukuran, codec, hash, dan path yang tidak valid.
- Ekstraksi tidak boleh menulis di luar direktori target.
- Symlink yang sudah ada di target tidak boleh dipakai untuk mengalihkan penulisan.
- Batas file, chunk, memori, output, dan waktu harus dapat dikonfigurasi.
- Decoder LZ harus tahan terhadap malformed stream dan decompression bomb.
- Tidak ada eksekusi kode, macro, atau script dari arsip.

## 26.3 Portabilitas

- Format biner menggunakan little-endian eksplisit.
- Path disimpan sebagai UTF-8 dan `/`.
- Reader harus memvalidasi representasi path terhadap aturan platform tujuan.
- Perbedaan permission dan timestamp antarplatform didokumentasikan.
- Binary release disediakan untuk target yang secara resmi didukung.

## 26.4 Kinerja

Target kinerja bukan jaminan sebelum benchmark. Implementasi harus mengukur:
- throughput create dan extract;
- throughput verify;
- rasio kompresi;
- rasio deduplikasi;
- penggunaan peak memory;
- waktu pembukaan/listing;
- performa file kecil dan file besar;
- performa dataset dengan konten berulang dan tidak berulang.

Tidak boleh mengorbankan validasi keamanan demi benchmark. Optimasi harus mempertahankan output canonical.

## 26.5 Durability

Writer harus menulis ke file sementara pada filesystem tujuan jika memungkinkan, flush dan sync sesuai opsi durability, kemudian rename atomik. Jika proses gagal, file final lama tidak boleh rusak atau tertimpa sebagian.

# 27. Strategi Pengujian Produk

## 27.1 Lapisan Pengujian

1. **Unit tests:** varint, Zigzag, BLAKE3 wrapper, Gear table, CDC, KolArk-LZ, metadata, index, path.
2. **Golden tests:** byte arsip dan payload codec yang sudah dibekukan.
3. **Round-trip tests:** file tunggal, banyak file, file kosong, file besar, binary, UTF-8 path.
4. **Cross-implementation:** Python writer → Rust reader; Rust writer → Python reader; byte equality Python writer ↔ Rust writer.
5. **Negative tests:** malformed header/footer, overflow, truncated record, duplicate path, invalid index, invalid varint, invalid LZ.
6. **Fuzzing:** parser header, footer, varint, column, index, LZ decoder, archive reader, extraction path.
7. **Property tests:** encode/decode round-trip, chunk reconstruction, dedup invariants, stable canonical output.
8. **Security tests:** traversal, symlink race, path collision, overwrite, decompression bomb, resource exhaustion.
9. **Performance tests:** dataset realistis dan sintetis.

## 27.2 Golden Vector Gate

Golden vector tidak boleh dianggap final hanya karena generator berhasil berjalan. Sebelum dibekukan, seluruh syarat berikut wajib terpenuhi:

- BLAKE3 known-answer tests lulus terhadap implementasi independen.
- Setiap offset dan ukuran dihitung ulang oleh parser kedua.
- Python dan Rust menghasilkan arsip byte-identik.
- Reader kedua implementasi dapat membaca arsip yang sama.
- Ekstraksi menghasilkan byte input identik.
- Manifest SHA-256 untuk fixture disimpan.
- Fixture memiliki deskripsi input dan versi generator.
- Fixture negatif disediakan untuk setiap kelas kesalahan utama.

Angka ukuran dan hash pada §19 merupakan data fixture yang harus diverifikasi ulang dengan test harness independen sebelum status release. Jika hasil pengujian berbeda, fixture diperbaiki melalui proses perubahan terkontrol; jangan mengubah implementasi agar sekadar cocok dengan angka yang belum diverifikasi.

## 27.3 Fuzzing dan Sanitasi

- Fuzz target tidak boleh menulis file di luar temporary directory.
- Setiap crash harus direproduksi sebagai regression test.
- CI menjalankan fuzz smoke test pada setiap perubahan dan sesi fuzz lebih panjang terjadwal.
- Jalankan Miri pada modul yang kompatibel dan sanitizer pada target yang didukung.
- Uji parser dengan batas integer, input kosong, satu byte, dan file terpotong di setiap boundary struktur.

# 28. Build, CI, dan Release Engineering

## 28.1 CI Wajib

Pipeline pull request minimal:

1. format check;
2. lint dengan warning policy yang ketat;
3. unit test;
4. integration test;
5. golden/conformance test;
6. dependency audit;
7. build CLI pada target utama;
8. dokumentasi dan contoh compile/test.

Pipeline release menambahkan:
- cross-platform build;
- cross-implementation interoperability;
- fuzz regression suite;
- benchmark report;
- SBOM;
- checksum artifact;
- signature artifact bila proses signing sudah tersedia;
- verifikasi reproducible build.

## 28.2 Artefak Distribusi

Rilis produk menyediakan:
- source archive;
- executable CLI untuk platform yang didukung;
- crate/library package;
- `spesifikasi.md`;
- README dan quick start;
- changelog;
- test vectors;
- manifest checksum;
- SBOM;
- license dan notices dependency.

## 28.3 Versioning

Pisahkan tiga nomor versi:

- **Format version:** angka di Header arsip; v1 bernilai `1`.
- **Specification revision:** revisi dokumen; tidak otomatis mengubah format.
- **Application version:** versi CLI/library mengikuti Semantic Versioning.

Perubahan yang mengubah interpretasi byte harus menaikkan format version atau menggunakan mekanisme extension yang telah distandardisasi pada versi berikutnya. Perubahan editorial tidak boleh diam-diam mengubah byte format.

# 29. Dokumentasi dan Pengalaman Pengguna

Dokumentasi produk minimal mencakup:

- README: manfaat, status, instalasi, contoh paling singkat.
- Quick Start: create, list, verify, extract.
- Format Specification: dokumen ini.
- CLI Reference: semua command, option, exit code.
- Library API: contoh pemakaian dan error handling.
- Security Guide: threat model, batasan hash, ekstraksi aman.
- Compatibility Matrix: format version dan versi implementasi.
- Troubleshooting: error umum dan pemulihan.
- Contributing Guide: build, test, lint, benchmark.
- Changelog: perubahan aplikasi dan format secara terpisah.

Dokumentasi tidak boleh mengklaim enkripsi, autentikasi penerbit, kompatibilitas ZIP/TAR, atau fitur yang belum benar-benar tersedia.

# 30. Threat Model

## 30.1 Aset yang Dilindungi

- Integritas file hasil ekstraksi.
- Batas direktori target.
- Ketersediaan memori, CPU, dan disk.
- Konsistensi arsip dan indeks.
- File yang sudah ada di filesystem pengguna.

## 30.2 Ancaman

- Arsip dimodifikasi atau terpotong.
- Offset dan panjang sengaja dibuat overflow.
- Metadata memicu alokasi besar.
- Stream LZ malformed atau menghasilkan output berlebihan.
- Path traversal melalui `..`, path absolut, separator alternatif, atau symlink.
- Path berbeda bertabrakan pada filesystem case-insensitive.
- Arsip mengandung referensi chunk invalid atau siklus logika yang tidak semestinya.
- File tujuan ditimpa tanpa izin.

## 30.3 Batas Jaminan

BLAKE3 unkeyed mendeteksi perubahan terhadap digest yang tersimpan, tetapi tidak membuktikan siapa pembuat arsip. Penyerang yang dapat mengubah seluruh arsip dapat menghitung ulang `hash_all`. KolArk v1 tidak memberikan kerahasiaan. Pengguna yang memerlukan autentikasi atau kerahasiaan harus menggunakan lapisan keamanan eksternal sampai versi yang mendukung fitur tersebut distandardisasi.

# 31. Kriteria Kesiapan Produk (Definition of Done)

KolArk baru boleh diberi label **Production Ready** jika seluruh gerbang berikut terpenuhi:

| Gerbang | Kriteria lulus |
|---|---|
| Format | Seluruh struktur biner dan seluruh field memiliki definisi final, tanpa kontradiksi |
| Gear CDC | Generator menghasilkan tepat 256 nilai `u32`; checksum tabel dibekukan dan diverifikasi dua bahasa |
| BLAKE3 | Known-answer tests lulus pada implementasi independen |
| KolArk-LZ | Grammar, encoder canonical, decoder, dan test vector final lulus |
| Metadata | Enam kolom diuji termasuk empty archive dan batas ukuran |
| Writer | Output canonical dan aman terhadap kegagalan I/O |
| Reader | Memvalidasi seluruh struktur dan fail closed |
| Extraction | Tidak ada path escape, symlink escape, atau overwrite tanpa izin |
| Interoperabilitas | Python/Rust cross-read dan byte-identical cross-write lulus |
| Golden vectors | Fixture valid/invalid dan checksum dibekukan |
| Fuzzing | Tidak ada crash yang belum ditriase; regression corpus berjalan di CI |
| Resource limits | Batas memori, output, jumlah file, chunk, dan waktu tersedia |
| Portabilitas | Target rilis dibangun dan diuji di CI |
| Supply chain | Dependency audit, lockfile, SBOM, checksum, dan provenance rilis tersedia |
| Dokumentasi | CLI/API/security/compatibility terdokumentasi |
| Release | Paket instalasi dan prosedur upgrade/rollback diuji |

**Status awal berdasarkan materi proyek yang tersedia:** spesifikasi format dan rancangan produk telah dirangkum, tetapi status Production Ready belum terbukti. Implementasi Rust Reader/Writer, verifikasi golden vector independen, interoperabilitas, fuzzing, dan release pipeline harus lulus sebelum klaim tersebut digunakan.

# 32. Roadmap Implementasi

## Fase 0 — Pembekuan Spesifikasi
- Tinjau seluruh struktur biner.
- Verifikasi semua ukuran, offset, dan urutan field.
- Bekukan Gear table melalui generator dan checksum.
- Finalisasi grammar KolArk-LZ dan fixture.
- Tetapkan fixture valid dan invalid.

**Exit gate:** tidak ada keputusan format yang masih ambigu.

## Fase 1 — Reference Codec
- BLAKE3 known-answer tests.
- Gear table generator.
- CDC splitter.
- KolArk-LZ encoder/decoder.
- Unit/property tests.

**Exit gate:** output codec deterministik dan fixture lintas bahasa disepakati.

## Fase 2 — Rust Reader
- Header/superblock/footer parser.
- Bounded reader dan checked offsets.
- Chunk index dan metadata parser.
- Hash verification.
- Safe extraction.

**Exit gate:** reader lulus seluruh fixture valid dan menolak fixture invalid.

## Fase 3 — Rust Writer
- Input enumeration canonical.
- CDC, dedup, compression.
- Metadata column writer.
- Final chunk index mapping.
- Footer/hash finalization.
- Atomic output.

**Exit gate:** writer menghasilkan arsip yang dibaca reader sendiri dan implementasi pembanding.

## Fase 4 — CLI dan Library API
- Create/list/info/verify/extract.
- Error code stabil.
- JSON output.
- Dokumentasi dan contoh.

**Exit gate:** alur pengguna end-to-end lulus pada platform target.

## Fase 5 — Hardening
- Fuzzing.
- Security review.
- Resource exhaustion tests.
- Performance benchmarks.
- Dependency/SBOM review.

**Exit gate:** tidak ada isu keamanan kritis/tinggi yang belum diselesaikan.

## Fase 6 — Release
- Reproducible build.
- Binary/source/library packages.
- Checksums dan provenance.
- Changelog dan compatibility matrix.
- Release candidate dan final release.

**Exit gate:** seluruh Definition of Done §31 terpenuhi.

# 33. Keputusan yang Tidak Boleh Diubah Secara Diam-Diam

Perubahan berikut memerlukan revisi spesifikasi dan fixture baru:

- magic dan format version;
- ukuran atau urutan Header, Superblock, Chunk Record, Column, Chunk Index, Footer;
- seed/konstanta/algoritma Gear table;
- state update dan boundary rule CDC;
- BLAKE3 mode atau cakupan hashing;
- grammar atau aturan canonical KolArk-LZ;
- encoding enam kolom;
- aturan deduplication dan collision;
- urutan canonical file, dictionary, dan index;
- makna chunk reference;
- aturan path dan ekstraksi.

Perubahan internal seperti struktur data map, thread pool, buffering, atau algoritma pencarian boleh dilakukan tanpa revisi format jika output canonical dan seluruh jaminan tetap identik.

# 34. Status Dokumen dan Klaim Kesesuaian

Dokumen ini menggabungkan:
- spesifikasi format biner KolArk v1 yang direvisi;
- persyaratan produk CLI dan library;
- arsitektur implementasi;
- keamanan, pengujian, CI, distribusi, dan kriteria rilis.

Label **Normative** berlaku untuk aturan format yang dinyatakan dengan MUST/MUST NOT/SHOULD/MAY. Bagian API/CLI menetapkan kontrak produk yang direkomendasikan dan dapat mengalami revisi aplikasi selama tidak mengubah byte format.

Tidak ada implementasi yang boleh menyatakan **KolArk v1 conforming** sebelum lulus suite konformansi. Tidak ada distribusi yang boleh menyatakan **Production Ready** sebelum seluruh Definition of Done §31 terpenuhi.

---

## Lampiran C — Checklist Implementasi Rilis

- [ ] Gear table tepat 256 entri `u32`.
- [ ] Checksum Gear table dibekukan.
- [ ] BLAKE3 known-answer tests lulus.
- [ ] CDC test vectors dibekukan.
- [ ] KolArk-LZ encode/decode vectors dibekukan.
- [ ] Varint dan Zigzag boundary tests lulus.
- [ ] Header/Superblock/Footer parser lulus.
- [ ] Column parser untuk keenam kolom lulus.
- [ ] Chunk Index parser dan collision test lulus.
- [ ] Empty archive dan empty file lulus.
- [ ] Python ↔ Rust cross-read lulus.
- [ ] Python ↔ Rust byte-identical writer lulus.
- [ ] Verify menolak setiap korupsi fixture.
- [ ] Extract menolak path traversal dan symlink escape.
- [ ] Resource limits aktif.
- [ ] CLI exit code stabil.
- [ ] CI, fuzzing, dependency audit aktif.
- [ ] SBOM, checksums, license notices tersedia.
- [ ] Dokumentasi dan changelog rilis lengkap.

**Akhir Spesifikasi Produk KolArk.**
