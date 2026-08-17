# Homebrew formula for noworries.
#
# This lives in a *tap* repo named `homebrew-noworries` (so users run
# `brew install <owner>/noworries/noworries`). Put this file at
# `Formula/noworries.rb` in that tap.
#
# The url/version/sha256 below are per-release. After each GitHub release,
# update `version` and the three `sha256` values (from the .sha256 files the
# release workflow uploads). This can be automated from the release workflow.
class Noworries < Formula
  desc "Ephemeral infra harness that lets an AI verify the changes it just made"
  homepage "https://github.com/guvense/noworries"
  version "0.1.0"
  license "MIT"

  BASE = "https://github.com/guvense/noworries/releases/download/v#{version}".freeze

  on_macos do
    on_arm do
      url "#{BASE}/noworries-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_AARCH64_APPLE_DARWIN"
    end
    on_intel do
      url "#{BASE}/noworries-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_X86_64_APPLE_DARWIN"
    end
  end

  on_linux do
    on_intel do
      url "#{BASE}/noworries-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_X86_64_LINUX_GNU"
    end
  end

  def install
    bin.install "noworries"
  end

  def caveats
    <<~EOS
      Install the /noworries skill for Claude Code (once):
        noworries install-command

      noworries needs Docker running when you use it.
    EOS
  end

  test do
    assert_match "noworries", shell_output("#{bin}/noworries --version")
  end
end
