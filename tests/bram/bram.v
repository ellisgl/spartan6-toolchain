// Block RAM test over the Mojo's USB serial port. Three memories with known
// initial contents (bram_init.vh, from tools/bram_data.py):
//   m0: 2048 x 8   (RAMB16)
//   m1:  512 x 32  (RAMB16, 36-bit-wide port)
//   m2: 1024 x 8
// Commands (little-endian):
//   'R' addr_lo addr_hi mem            -> 4 bytes: data (zero-extended)
//   'W' addr_lo addr_hi mem d0 d1 d2 d3 -> 'K'
// Checked by tools/bram-test.py.
module bram (
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

    reg [7:0]  m0 [0:2047];
    reg [31:0] m1 [0:511];
    reg [7:0]  m2 [0:1023];
`include "bram_init.vh"

    // Command buffer: cmd, addr (2), mem, data (4).
    reg [63:0] cmd = 0;
    reg [3:0]  ncmd = 0;
    wire [7:0]  op   = cmd[7:0];
    wire [15:0] addr = cmd[23:8];
    wire [7:0]  sel  = cmd[31:24];
    wire [31:0] wdat = cmd[63:32];

    reg        we0 = 0, we1 = 0, we2 = 0;
    reg [7:0]  q0 = 0, q2 = 0;
    reg [31:0] q1 = 0;
    always @(posedge clk) begin
        if (we0) m0[addr[10:0]] <= wdat[7:0];
        q0 <= m0[addr[10:0]];
    end
    always @(posedge clk) begin
        if (we1) m1[addr[8:0]] <= wdat;
        q1 <= m1[addr[8:0]];
    end
    always @(posedge clk) begin
        if (we2) m2[addr[9:0]] <= wdat[7:0];
        q2 <= m2[addr[9:0]];
    end

    reg [31:0] out = 0;
    reg [2:0]  nout = 0;
    reg [2:0]  phase = 0;   // 0 idle, 1-2 read wait, 3 answer, 4 write
    wire       tx_taken;
    uart_tx #(.GAP_BITS(10)) utx (.clk(clk), .avr_busy(avr_rx_busy), .send(nout != 0 && ready),
                                  .data(out[7:0]), .taken(tx_taken), .tx_pin(avr_rx));

    always @(posedge clk) begin
        we0 <= 1'b0;
        we1 <= 1'b0;
        we2 <= 1'b0;
        if (rx_valid && phase == 0) begin
            if (ncmd == 0)
                cmd <= {56'h0, rx_data};
            else
                cmd[ncmd * 8 +: 8] <= rx_data;
            ncmd <= ncmd + 1'b1;
        end
        if (phase == 0 && ncmd == 4 && op == "R") begin
            phase <= 1;
            ncmd  <= 0;
        end else if (phase == 0 && ncmd == 8 && op == "W") begin
            phase <= 4;
            ncmd  <= 0;
        end else if (phase == 0 && ncmd != 0 && op != "R" && op != "W") begin
            ncmd <= 0;  // unknown command: resync
        end
        case (phase)
            1, 2: phase <= phase + 1'b1;  // let the BRAM output settle
            3: begin
                out   <= sel == 0 ? {24'h0, q0} : sel == 1 ? q1 : {24'h0, q2};
                nout  <= 4;
                phase <= 0;
            end
            4: begin
                we0   <= sel == 0;
                we1   <= sel == 1;
                we2   <= sel == 2;
                out   <= "K";
                nout  <= 1;
                phase <= 0;
            end
            default: ;
        endcase
        if (tx_taken) begin
            out  <= {8'h00, out[31:8]};
            nout <= nout - 1'b1;
        end
    end

    assign led = q0;
endmodule
