project('benchmarks', 'cpp', default_options : ['cpp_std=c++14'])

google_benchmark = subproject('google-benchmark')
google_benchmark_dep = google_benchmark.get_variable('google_benchmark_dep')

stb_truetype = subproject('stb_truetype')
stb_truetype_dep = stb_truetype.get_variable('stb_truetype_dep')

freetype_dep = dependency('freetype2', version : '>=2.8')

# maybe there is a better way
ttf_parser_dep = meson.get_compiler('cpp').find_library('ttfparser',
    dirs : [meson.current_source_dir() + '/../../c-api/target/release'])

executable('outline', 'outline.cc',
    dependencies : [google_benchmark_dep, freetype_dep, stb_truetype_dep, ttf_parser_dep],
    include_directories : ['../../c-api'],
    override_options : ['cpp_std=c++14'])
