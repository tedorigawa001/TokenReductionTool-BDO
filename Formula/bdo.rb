# typed: false
# frozen_string_literal: true

# Homebrew formula for bdo (Bushido) - fork of rtk (Rust Token Killer)
# To install: brew tap tedorigawa001/tap && brew install bdo
# NOTE: tap repo assumed to be tedorigawa001/homebrew-tap. The release URLs
# below expect bdo-<target>.tar.gz artifacts attached to the GitHub releases of
# tedorigawa001/TokenReductionTool; create those releases before publishing.
class Bdo < Formula
  desc "High-performance CLI proxy to minimize LLM token consumption"
  homepage "https://github.com/tedorigawa001/TokenReductionTool"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    on_intel do
      url "https://github.com/tedorigawa001/TokenReductionTool/releases/download/v#{version}/bdo-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_INTEL"
    end

    on_arm do
      url "https://github.com/tedorigawa001/TokenReductionTool/releases/download/v#{version}/bdo-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/tedorigawa001/TokenReductionTool/releases/download/v#{version}/bdo-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_INTEL"
    end

    on_arm do
      url "https://github.com/tedorigawa001/TokenReductionTool/releases/download/v#{version}/bdo-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_ARM"
    end
  end

  def install
    bin.install "bdo"
  end

  test do
    assert_match "bdo #{version}", shell_output("#{bin}/bdo --version")
  end
end
