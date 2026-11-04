// PLL_ADV with its dynamic reconfiguration port (DRP) driven from logic,
// over the Mojo's serial port. From the 50 MHz board clock: VCO 500 MHz,
// CLKOUT0 / 4 = 125 MHz. DCLK is the board clock, so its BUFG goes on
// BUFGMUX 0..7 (the only global lines that reach DCLK's interconnect input),
// fed from the pad through general routing; DADDR, DI, DWE and DEN are set
// on the same edge, as in Xilinx's DRP reference design.
//   'R' a        -> 2 bytes   DRP read of address a[4:0] (DO, little-endian)
//   'W' a lo hi  -> 'K'       DRP write of {hi, lo} to address a[4:0]
//   'X' r        -> 'K'       PLL RST = r[0] (hold it in reset while writing)
//   'C'          -> 5 bytes   CLKOUT0 cycles in 65536 board clocks (LE),
//                             then {7'b0, LOCKED}
// Checked by tools/plldrp-test.py.
module plldrp (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       cclk,
    input  wire       avr_tx,
    output wire       avr_rx,
    input  wire       avr_rx_busy,
    output wire [7:0] led
);
    wire ready;
    mojo_ready rdy (.clk(clk), .cclk(cclk), .ready(ready));
    wire [7:0] rx_data;
    wire       rx_valid;
    uart_rx urx (.clk(clk), .rx_pin(avr_tx), .data(rx_data), .valid(rx_valid));

    // --- PLL with DRP ---
    reg         pll_rst = 0;
    reg  [4:0]  daddr = 0;
    reg  [15:0] di = 0;
    reg         den = 0, dwe = 0;
    wire [15:0] dout;
    wire        drdy, fb, o0, locked;
    PLL_ADV #(
        .CLKIN1_PERIOD(20.0),
        .CLKFBOUT_MULT(10),
        .CLKOUT0_DIVIDE(4),
        .COMPENSATION("INTERNAL")
    ) pll (
        .CLKIN1(clk), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb), .RST(pll_rst), .REL(1'b0),
        .DCLK(clk), .DEN(den), .DWE(dwe), .DADDR(daddr), .DI(di), .DO(dout), .DRDY(drdy),
        .CLKFBOUT(fb), .CLKOUT0(o0), .LOCKED(locked)
    );
    wire g0;
    BUFG bufg_0 (.I(o0), .O(g0));

    // --- CLKOUT0 frequency measurement ---
    reg [16:0] wcnt = 0;
    reg        window = 0, start = 0, report = 0;
    reg [9:0]  settle = 0;
    always @(posedge clk) begin
        report <= 1'b0;
        if (start && !window && settle == 0) begin
            window <= 1'b1;
            wcnt   <= 0;
        end else if (window) begin
            wcnt <= wcnt + 1'b1;
            if (wcnt == 17'd65535) begin
                window <= 1'b0;
                settle <= 1;
            end
        end else if (settle != 0) begin
            settle <= settle + 1'b1;
            if (&settle) begin
                settle <= 0;
                report <= 1'b1;
            end
        end
    end
    reg [31:0] n0 = 0;
    reg [2:0]  w0 = 0;
    always @(posedge g0) begin
        w0 <= {w0[1:0], window};
        if (w0[2:1] == 2'b01) n0 <= 0;
        else if (w0[1]) n0 <= n0 + 1'b1;
    end

    // --- command handling ---
    reg [7:0]  op = 0;
    reg [1:0]  nargs = 0, got = 0;
    reg [23:0] args = 0;
    reg        busy = 0;     // waiting for DRDY
    reg [39:0] out = 0;
    reg [2:0]  nout = 0;
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));
    always @(posedge clk) begin
        den   <= 1'b0;
        dwe   <= 1'b0;
        start <= 1'b0;
        if (busy && drdy) begin
            busy <= 1'b0;
            if (op == "R") begin out <= {24'd0, dout}; nout <= 2; end
            else           begin out <= "K";           nout <= 1; end
        end else if (report) begin
            out  <= {7'b0, locked, n0};
            nout <= 5;
        end else if (rx_valid && !busy && nout == 0) begin
            if (nargs == 0) begin
                // Unknown bytes are ignored (stray AVR bytes at startup).
                case (rx_data)
                    "R": begin op <= rx_data; nargs <= 1; got <= 0; end
                    "W": begin op <= rx_data; nargs <= 3; got <= 0; end
                    "X": begin op <= rx_data; nargs <= 1; got <= 0; end
                    "C": start <= 1'b1;
                    default: ;
                endcase
            end else begin
                args <= {rx_data, args[23:8]};
                got  <= got + 1'b1;
                if (got + 1'b1 == nargs) begin
                    nargs <= 0;
                    // The last byte arrives now: args is {rx_data, args[23:8]}.
                    if (op == "R") begin
                        daddr <= rx_data[4:0]; den <= 1'b1; busy <= 1'b1;
                    end else if (op == "W") begin
                        daddr <= args[12:8]; di <= {rx_data, args[23:16]};
                        den <= 1'b1; dwe <= 1'b1; busy <= 1'b1;
                    end else begin
                        pll_rst <= rx_data[0]; out <= "K"; nout <= 1;
                    end
                end
            end
        end else if (tx_taken) begin
            out  <= {8'h00, out[39:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = {pll_rst, 6'b0, locked};
endmodule
