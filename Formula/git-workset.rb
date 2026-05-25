class GitWorkset < Formula
  desc "Named sparse-checkout profiles for git worktrees"
  homepage "https://lauripiispanen.github.io/git-workset/"
  version "VERSION_PLACEHOLDER"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/lauripiispanen/git-workset/releases/download/v#{version}/git-workset-x86_64-apple-darwin.tar.gz"
      sha256 "SHA256_X86_DARWIN"
    end

    on_arm do
      url "https://github.com/lauripiispanen/git-workset/releases/download/v#{version}/git-workset-aarch64-apple-darwin.tar.gz"
      sha256 "SHA256_ARM_DARWIN"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/lauripiispanen/git-workset/releases/download/v#{version}/git-workset-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "SHA256_X86_LINUX"
    end

    on_arm do
      url "https://github.com/lauripiispanen/git-workset/releases/download/v#{version}/git-workset-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "SHA256_ARM_LINUX"
    end
  end

  def install
    bin.install "git-workset"
  end

  test do
    system "#{bin}/git-workset", "--version"
  end
end
