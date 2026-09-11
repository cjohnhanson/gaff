# The release workflow renders this template into the tap on every tag:
# it fills the version and the four checksums from the build's own
# archives, then pushes the result to cjohnhanson/homebrew-tap. Edit
# the formula here, never in the tap.
class Gaff < Formula
  desc "A context-lifecycle handler for coding agents"
  homepage "https://github.com/cjohnhanson/gaff"
  license "MIT"
  version "0.0.0"

  on_macos do
    on_arm do
      url "https://github.com/cjohnhanson/gaff/releases/download/v#{version}/gaff-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_DARWIN_ARM64"
    end
    on_intel do
      url "https://github.com/cjohnhanson/gaff/releases/download/v#{version}/gaff-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_DARWIN_X64"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cjohnhanson/gaff/releases/download/v#{version}/gaff-v#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "PLACEHOLDER_LINUX_ARM64"
    end
    on_intel do
      url "https://github.com/cjohnhanson/gaff/releases/download/v#{version}/gaff-v#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "PLACEHOLDER_LINUX_X64"
    end
  end

  def install
    bin.install "gaff"
    # Both names install. The short name exists because a registry name
    # was taken, and the typed name is gaff.
    bin.install_symlink bin/"gaff" => "gaffr"
    man1.install "man/gaff.1" if File.exist?("man/gaff.1")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/gaff --version")
  end
end
