cmake_minimum_required(VERSION 3.21)

cmake_policy(SET CMP0091 NEW)
project(escode_native_ugrep LANGUAGES C CXX)

if(NOT MSVC)
  message(FATAL_ERROR "ESCode Windows ugrep must be built with MSVC")
endif()

foreach(source_name UGREP PCRE2 ZLIB BZIP2 ZSTD BROTLI)
  if(NOT DEFINED ESCODE_${source_name}_SOURCE_DIR)
    message(FATAL_ERROR "Missing ESCODE_${source_name}_SOURCE_DIR")
  endif()
endforeach()

if(NOT DEFINED ESCODE_UGREP_OUTPUT_DIR)
  message(FATAL_ERROR "Missing ESCODE_UGREP_OUTPUT_DIR")
endif()

set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)

# 随机构建目录不能进入最终 PE，否则同版本工具无法跨 CLI 复用同一份 runtime cache。
if(DEFINED ESCODE_BUILD_PATH)
  add_compile_options("/pathmap:${ESCODE_BUILD_PATH}=escode-native-search")
endif()

set(BUILD_STATIC_LIBS ON CACHE BOOL "" FORCE)
set(PCRE2_BUILD_PCRE2_8 ON CACHE BOOL "" FORCE)
set(PCRE2_BUILD_PCRE2_16 OFF CACHE BOOL "" FORCE)
set(PCRE2_BUILD_PCRE2_32 OFF CACHE BOOL "" FORCE)
set(PCRE2_BUILD_PCRE2GREP OFF CACHE BOOL "" FORCE)
set(PCRE2_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(PCRE2_SHOW_REPORT OFF CACHE BOOL "" FORCE)
set(PCRE2_STATIC_RUNTIME ON CACHE BOOL "" FORCE)
set(PCRE2_SUPPORT_JIT ON CACHE BOOL "" FORCE)
add_subdirectory("${ESCODE_PCRE2_SOURCE_DIR}" pcre2 EXCLUDE_FROM_ALL)

set(SKIP_INSTALL_ALL ON CACHE BOOL "" FORCE)
add_subdirectory("${ESCODE_ZLIB_SOURCE_DIR}" zlib EXCLUDE_FROM_ALL)

add_library(escode_bzip2 STATIC
  "${ESCODE_BZIP2_SOURCE_DIR}/blocksort.c"
  "${ESCODE_BZIP2_SOURCE_DIR}/huffman.c"
  "${ESCODE_BZIP2_SOURCE_DIR}/crctable.c"
  "${ESCODE_BZIP2_SOURCE_DIR}/randtable.c"
  "${ESCODE_BZIP2_SOURCE_DIR}/compress.c"
  "${ESCODE_BZIP2_SOURCE_DIR}/decompress.c"
  "${ESCODE_BZIP2_SOURCE_DIR}/bzlib.c"
)
target_compile_definitions(escode_bzip2 PRIVATE _CRT_SECURE_NO_WARNINGS)
target_include_directories(escode_bzip2 PUBLIC "${ESCODE_BZIP2_SOURCE_DIR}")

set(ZSTD_BUILD_CONTRIB OFF CACHE BOOL "" FORCE)
set(ZSTD_BUILD_PROGRAMS OFF CACHE BOOL "" FORCE)
set(ZSTD_BUILD_SHARED OFF CACHE BOOL "" FORCE)
set(ZSTD_BUILD_STATIC ON CACHE BOOL "" FORCE)
set(ZSTD_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(ZSTD_USE_STATIC_RUNTIME ON CACHE BOOL "" FORCE)
add_subdirectory("${ESCODE_ZSTD_SOURCE_DIR}/build/cmake" zstd EXCLUDE_FROM_ALL)

set(BROTLI_BUNDLED_MODE ON CACHE BOOL "" FORCE)
add_subdirectory("${ESCODE_BROTLI_SOURCE_DIR}" brotli EXCLUDE_FROM_ALL)

set(ESCODE_VIIZ_SOURCE_DIR "${ESCODE_UGREP_SOURCE_DIR}/lzma/C")
add_library(escode_viiz STATIC
  "${ESCODE_VIIZ_SOURCE_DIR}/viizip.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zAlloc.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zArcIn.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zBuf.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zBuf2.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zCrc.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zCrcOpt.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zDec.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zFile.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/7zStream.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/Bcj2.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/Bra.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/Bra86.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/BraIA64.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/CpuArch.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/Delta.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/Lzma2Dec.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/LzmaDec.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/Ppmd7.c"
  "${ESCODE_VIIZ_SOURCE_DIR}/Ppmd7Dec.c"
)
target_compile_definitions(escode_viiz PRIVATE
  Z7_PPMD_SUPPORT
  Z7_EXTRACT_ONLY
  NDEBUG
  _CRT_SECURE_NO_WARNINGS
)
target_include_directories(escode_viiz PUBLIC "${ESCODE_VIIZ_SOURCE_DIR}")

set(ESCODE_REFLEX_SOURCES
  "${ESCODE_UGREP_SOURCE_DIR}/lib/block_scripts.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/composer.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/convert.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/debug.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/error.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/input.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/language_scripts.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/letter_case.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/letter_scripts.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/matcher.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/pattern.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/posix.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/simd.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/unicode.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/lib/utf8.cpp"
)

add_library(escode_reflex STATIC ${ESCODE_REFLEX_SOURCES})
target_compile_definitions(escode_reflex PRIVATE WITH_NO_CODEGEN)

add_executable(ugrep
  "${ESCODE_UGREP_SOURCE_DIR}/src/ugrep.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/src/cnf.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/src/glob.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/src/output.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/src/query.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/src/screen.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/src/stats.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/src/vkey.cpp"
  "${ESCODE_UGREP_SOURCE_DIR}/src/zopen.c"
)

add_library(escode_ugrep_contract INTERFACE)
target_compile_definitions(escode_ugrep_contract INTERFACE
  HAVE_LIBBROTLI
  HAVE_LIBBZ2
  HAVE_LIBZ
  HAVE_LIBZSTD
  HAVE_PCRE2
  NDEBUG
  PCRE2_STATIC
  WITH_COLOR
  WITH_NO_INDENT
  _CONSOLE
  _CRT_NONSTDC_NO_WARNINGS
  _CRT_SECURE_NO_WARNINGS
)
if(CMAKE_GENERATOR_PLATFORM STREQUAL "ARM64")
  target_compile_definitions(escode_ugrep_contract INTERFACE HAVE_NEON)
else()
  target_compile_definitions(escode_ugrep_contract INTERFACE HAVE_SSE2)
endif()
target_compile_features(escode_ugrep_contract INTERFACE cxx_std_17)
# ugrep 固定源码是无 BOM UTF-8；中文 Windows 按 CP936 解析时会触发 C4819/C1070，并改写 UTF-8 字符串字面量。
target_compile_options(escode_ugrep_contract INTERFACE
  /utf-8
  $<$<COMPILE_LANGUAGE:CXX>:/EHsc>
)
target_include_directories(escode_ugrep_contract INTERFACE
  "${ESCODE_UGREP_SOURCE_DIR}/include"
  "${ESCODE_UGREP_SOURCE_DIR}/src"
  "${ESCODE_VIIZ_SOURCE_DIR}"
)

target_link_libraries(escode_reflex PRIVATE escode_ugrep_contract pcre2-8-static)
target_link_libraries(ugrep PRIVATE
  escode_ugrep_contract
  escode_reflex
  escode_viiz
  pcre2-8-static
  zlibstatic
  escode_bzip2
  libzstd_static
  brotlienc
  brotlidec
  brotlicommon
)
target_link_options(ugrep PRIVATE /Brepro)
set_target_properties(ugrep PROPERTIES
  RUNTIME_OUTPUT_DIRECTORY "${ESCODE_UGREP_OUTPUT_DIR}"
  RUNTIME_OUTPUT_DIRECTORY_RELEASE "${ESCODE_UGREP_OUTPUT_DIR}"
)
