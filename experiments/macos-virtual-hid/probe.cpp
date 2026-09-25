// A bounded Universal Control compatibility probe; not a Lan Mouse backend.
#include <atomic>
#include <chrono>
#include <csignal>
#include <iostream>
#include <string>
#include <thread>
#include <unistd.h>
#include <pqrs/karabiner/driverkit/virtual_hid_device_service.hpp>

namespace service = pqrs::karabiner::driverkit::virtual_hid_device_service;
namespace report = pqrs::karabiner::driverkit::virtual_hid_device_driver::hid_report;
using namespace std::chrono_literals;
static volatile std::sig_atomic_t stopped = 0;

int main(int argc, char** argv) {
  if (argc < 2 || argc > 3) {
    std::cout << "Usage: virtual-hid-probe check|keyboard|left|right|up|down [--dry-run]\n"
              << "keyboard types hidtest once, without Return; directions move 400 units.\n"
              << "Active tests wait 8 seconds so you can focus a blank note. Ctrl-C cancels.\n";
    return argc == 1 ? 0 : 2;
  }
  const std::string mode = argv[1];
  if (mode != "check" && mode != "keyboard" && mode != "left" && mode != "right" &&
      mode != "up" && mode != "down") {
    std::cerr << "Unknown mode\n";
    return 2;
  }
  const bool dry_run = argc == 3 && std::string(argv[2]) == "--dry-run";
  if (argc == 3 && !dry_run) return 2;
  if (dry_run) {
    std::cout << "Plan: " << mode << "; driver 1.8.0, client protocol 7; "
              << "8-second countdown; bounded reports; release and destroy test devices.\n";
    return 0;
  }
  if (geteuid() != 0) {
    std::cerr << "Karabiner's virtual-HID service requires root. Run the supplied run.sh with sudo.\n";
    return 2;
  }
  if (mode != "check" && !isatty(STDIN_FILENO)) {
    std::cerr << "Active tests require an interactive terminal.\n";
    return 2;
  }
  std::signal(SIGINT, [](int) { stopped = 1; });
  std::signal(SIGTERM, [](int) { stopped = 1; });
  std::atomic<bool> keyboard_ready{false}, pointing_ready{false}, failed{false};
  pqrs::dispatcher::extra::initialize_shared_dispatcher();
  int result = 0;
  {
    service::client client;
    client.warning_reported.connect([](const auto& message) { std::cerr << message << '\n'; });
    client.connected.connect([&] {
      std::cout << "Connected to virtual-HID daemon\n";
      service::virtual_hid_keyboard_parameters parameters;
      parameters.set_country_code(pqrs::hid::country_code::us);
      client.async_virtual_hid_keyboard_initialize(parameters);
      client.async_virtual_hid_pointing_initialize();
    });
    client.virtual_hid_keyboard_ready.connect([&](bool ready) { keyboard_ready = ready; });
    client.virtual_hid_pointing_ready.connect([&](bool ready) { pointing_ready = ready; });
    client.driver_version_mismatched.connect([&](bool mismatch) {
      if (mismatch) { std::cerr << "Driver version mismatch\n"; failed = true; }
    });
    client.closed.connect([&] { failed = true; });
    client.error_occurred.connect([&](const auto& error) { std::cerr << error << '\n'; failed = true; });
    client.connect_failed.connect([](const auto& error) { std::cerr << "Connecting: " << error << '\n'; });
    client.async_start();
    const auto deadline = std::chrono::steady_clock::now() + 10s;
    while (!stopped && !failed && !(keyboard_ready && pointing_ready) &&
           std::chrono::steady_clock::now() < deadline) {
      std::this_thread::sleep_for(20ms);
    }
    if (!(keyboard_ready && pointing_ready) || failed || stopped) {
      std::cerr << "Test devices did not become ready; no test input sent.\n";
      result = 1;
    } else {
      std::cout << "READY: virtual keyboard and pointing device\n" << std::flush;
      if (mode != "check") {
        std::cout << "Focus a blank note (keyboard), or position the pointer just inside the\n"
                     "Mac's iPad-facing edge (direction). Input starts in 8 seconds.\n" << std::flush;
        for (int i = 0; i < 80 && !stopped && !failed; ++i) std::this_thread::sleep_for(100ms);
        if (mode == "keyboard") {
          for (uint16_t usage : {11, 12, 7, 23, 8, 22, 23}) { // hidtest, USB HID usages
            if (stopped || failed) break;
            report::keyboard_input down;
            down.keys.insert(usage);
            client.async_post_report(down);
            std::this_thread::sleep_for(40ms);
            client.async_post_report(report::keyboard_input{});
            std::this_thread::sleep_for(100ms);
          }
        } else {
          for (int i = 0; i < 100 && !stopped && !failed; ++i) {
            report::pointing_input move;
            move.x = static_cast<uint8_t>(mode == "left" ? -4 : mode == "right" ? 4 : 0);
            move.y = static_cast<uint8_t>(mode == "up" ? -4 : mode == "down" ? 4 : 0);
            client.async_post_report(move);
            std::this_thread::sleep_for(20ms);
          }
        }
        std::cout << "Reports submitted. Observe the Mac/iPad to determine whether input arrived.\n";
      }
      if (stopped || failed) result = 1;
    }
    // Explicitly release keys/buttons before removing only this client's devices.
    client.async_post_report(report::keyboard_input{});
    client.async_post_report(report::pointing_input{});
    std::this_thread::sleep_for(200ms);
    client.async_virtual_hid_keyboard_terminate();
    client.async_virtual_hid_pointing_terminate();
    std::this_thread::sleep_for(200ms);
    client.async_stop();
  }
  pqrs::dispatcher::extra::terminate_shared_dispatcher();
  return result;
}
