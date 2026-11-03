# frozen_string_literal: true


desc "Sync assets/express-grammar.parg from the expressir checkout's edition-2 grammar"
task :"expressir:grammar:sync" do
  # The edition-2 .parg source is authored in lutaml/expressir
  # (grammar-parg/express-grammar.parg); this repo embeds a copy.
  # Resolve the expressir checkout relative to this repo (sibling
  # checkout) or the EXPRESSIR_DIR env var.
  candidates = [
    ENV["EXPRESSIR_DIR"],
    File.expand_path("../expressir", __dir__),
  ].compact
  source = candidates
    .map { |dir| File.join(dir, "grammar-parg", "express-grammar.parg") }
    .find { |p| File.exist?(p) }
  raise "expressir grammar source not found (set EXPRESSIR_DIR)" unless source

  dest = File.expand_path("assets/express-grammar.parg", __dir__)
  FileUtils.cp(source, dest)
  puts "copied #{source} -> #{dest}"
end
require "fileutils"
